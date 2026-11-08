//! Account store: users, password hashes (Argon2id), OAuth identities and
//! login sessions, kept in SQLite. Session tokens are random 32-byte values;
//! only their SHA-256 hash is stored.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MIN_PASSWORD: usize = 12;
pub const MAX_PASSWORD: usize = 256;
pub const MIN_NAME: usize = 3;
pub const MAX_NAME: usize = 24;
pub const SESSION_DAYS: i64 = 30;
/// How long a password reset link works.
pub const RESET_MINUTES: i64 = 30;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: Option<String>,
    pub has_password: bool,
    pub providers: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum AuthError {
    /// Bad input; the message is shown to the user.
    Invalid(String),
    /// Name or email already in use.
    Taken(String),
    /// Wrong email or password.
    BadCredentials,
    Internal(String),
}

impl AuthError {
    pub fn message(&self) -> String {
        match self {
            AuthError::Invalid(m) | AuthError::Taken(m) => m.clone(),
            AuthError::BadCredentials => "Wrong email or password.".into(),
            AuthError::Internal(_) => "Something went wrong. Try again.".into(),
        }
    }
}

impl From<rusqlite::Error> for AuthError {
    fn from(e: rusqlite::Error) -> Self {
        AuthError::Internal(e.to_string())
    }
}

pub struct Accounts {
    db: Mutex<Connection>,
    /// Hash used when the email is unknown, so a miss costs the same as a wrong password.
    dummy_hash: String,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).expect("os randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hash_password(pw: &str) -> Result<String, AuthError> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).expect("os randomness");
    let salt = SaltString::encode_b64(&raw).map_err(|e| AuthError::Internal(e.to_string()))?;
    Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| AuthError::Internal(e.to_string()))
}

fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash).map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok()).unwrap_or(false)
}

/// Player names: 3–24 characters of letters, digits, spaces, `_`, `-` and `.`.
pub fn check_name(raw: &str) -> Result<String, AuthError> {
    let n: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let len = n.chars().count();
    if len < MIN_NAME || len > MAX_NAME {
        return Err(AuthError::Invalid(format!("Player names are {MIN_NAME} to {MAX_NAME} characters.")));
    }
    if !n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.')) {
        return Err(AuthError::Invalid("Player names can use letters, numbers, spaces, _ - and .".into()));
    }
    Ok(n)
}

pub fn check_email(raw: &str) -> Result<String, AuthError> {
    let e = raw.trim().to_lowercase();
    let ok = e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control())
        && e.split_once('@').map(|(local, domain)| !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')).unwrap_or(false);
    if ok {
        Ok(e)
    } else {
        Err(AuthError::Invalid("Enter a valid email address.".into()))
    }
}

pub fn check_password(pw: &str) -> Result<(), AuthError> {
    let n = pw.chars().count();
    if n < MIN_PASSWORD {
        return Err(AuthError::Invalid(format!("Passwords need at least {MIN_PASSWORD} characters.")));
    }
    if n > MAX_PASSWORD {
        return Err(AuthError::Invalid(format!("Passwords can be at most {MAX_PASSWORD} characters.")));
    }
    Ok(())
}

impl Accounts {
    pub fn open(path: &Path) -> Result<Accounts, String> {
        let c = Connection::open(path).map_err(|e| format!("opening {}: {e} (the server needs write access there)", path.display()))?;
        Self::init(c)
    }

    pub fn in_memory() -> Accounts {
        Self::init(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn init(c: Connection) -> Result<Accounts, String> {
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE,
                email TEXT UNIQUE COLLATE NOCASE,
                pw_hash TEXT,
                created INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS identities (
                provider TEXT NOT NULL,
                subject TEXT NOT NULL,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                PRIMARY KEY (provider, subject)
             );
             CREATE TABLE IF NOT EXISTS sessions (
                token_hash BLOB PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                expires INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS reset_tokens (
                token_hash BLOB PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                expires INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        let dummy_hash = hash_password(&random_hex(16)).map_err(|e| e.message())?;
        Ok(Accounts { db: Mutex::new(c), dummy_hash })
    }

    fn user_by_id(c: &Connection, id: i64) -> Result<Option<User>, AuthError> {
        let row = c.query_row("SELECT id, name, email, pw_hash IS NOT NULL FROM users WHERE id = ?1", [id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, bool>(3)?))).optional()?;
        let Some((id, name, email, has_password)) = row else { return Ok(None) };
        let mut st = c.prepare("SELECT provider FROM identities WHERE user_id = ?1 ORDER BY provider")?;
        let providers = st.query_map([id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(Some(User { id, name, email, has_password, providers }))
    }

    pub fn user(&self, id: i64) -> Option<User> {
        Self::user_by_id(&self.db.lock().unwrap(), id).ok().flatten()
    }

    fn name_taken(c: &Connection, name: &str, except: Option<i64>) -> Result<bool, AuthError> {
        Ok(c.query_row("SELECT id FROM users WHERE name = ?1", [name], |r| r.get::<_, i64>(0)).optional()?.map(|id| Some(id) != except).unwrap_or(false))
    }

    /// Create a password account. Hashing is slow on purpose; call from a blocking task.
    pub fn sign_up(&self, name: &str, email: &str, password: &str) -> Result<User, AuthError> {
        let name = check_name(name)?;
        let email = check_email(email)?;
        check_password(password)?;
        let hash = hash_password(password)?;
        let c = self.db.lock().unwrap();
        if Self::name_taken(&c, &name, None)? {
            return Err(AuthError::Taken("That player name is taken.".into()));
        }
        let exists: Option<i64> = c.query_row("SELECT id FROM users WHERE email = ?1", [&email], |r| r.get(0)).optional()?;
        if exists.is_some() {
            return Err(AuthError::Taken("An account with that email already exists. Sign in instead.".into()));
        }
        c.execute("INSERT INTO users (name, email, pw_hash, created) VALUES (?1, ?2, ?3, ?4)", params![name, email, hash, now()])?;
        let id = c.last_insert_rowid();
        Ok(Self::user_by_id(&c, id)?.expect("just inserted"))
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<User, AuthError> {
        let email = email.trim().to_lowercase();
        let row: Option<(i64, Option<String>)> = {
            let c = self.db.lock().unwrap();
            c.query_row("SELECT id, pw_hash FROM users WHERE email = ?1", [&email], |r| Ok((r.get(0)?, r.get(1)?))).optional()?
        };
        match row {
            Some((id, Some(hash))) if verify_password(password, &hash) => self.user(id).ok_or(AuthError::BadCredentials),
            Some((_, Some(_))) => Err(AuthError::BadCredentials),
            _ => {
                let _ = verify_password(password, &self.dummy_hash);
                Err(AuthError::BadCredentials)
            }
        }
    }

    /// Sign in with an external identity. Links to an existing account when the
    /// provider vouches for the same email; otherwise creates a new account.
    pub fn oauth_login(&self, provider: &str, subject: &str, email: Option<&str>, email_verified: bool, display_name: &str) -> Result<User, AuthError> {
        let c = self.db.lock().unwrap();
        let linked: Option<i64> = c.query_row("SELECT user_id FROM identities WHERE provider = ?1 AND subject = ?2", [provider, subject], |r| r.get(0)).optional()?;
        if let Some(id) = linked {
            return Self::user_by_id(&c, id)?.ok_or_else(|| AuthError::Internal("dangling identity".into()));
        }
        let email = email.and_then(|e| check_email(e).ok());
        let by_email: Option<i64> = match (&email, email_verified) {
            (Some(e), true) => c.query_row("SELECT id FROM users WHERE email = ?1", [e], |r| r.get(0)).optional()?,
            _ => None,
        };
        let id = match by_email {
            Some(id) => id,
            None => {
                let base = check_name(display_name).unwrap_or_else(|_| format!("{}Player", capitalize(provider)));
                let base: String = base.chars().take(MAX_NAME - 4).collect();
                let mut name = base.clone();
                let mut n = 1;
                while Self::name_taken(&c, &name, None)? {
                    n += 1;
                    name = format!("{base}{n}");
                }
                // Only keep the email if nobody else owns it (an unverified match must not take it over).
                let keep_email = email.filter(|e| c.query_row("SELECT 1 FROM users WHERE email = ?1", [e], |_| Ok(())).optional().ok().flatten().is_none());
                c.execute("INSERT INTO users (name, email, pw_hash, created) VALUES (?1, ?2, NULL, ?3)", params![name, keep_email, now()])?;
                c.last_insert_rowid()
            }
        };
        c.execute("INSERT INTO identities (provider, subject, user_id) VALUES (?1, ?2, ?3)", params![provider, subject, id])?;
        Ok(Self::user_by_id(&c, id)?.expect("user exists"))
    }

    pub fn rename(&self, id: i64, name: &str) -> Result<User, AuthError> {
        let name = check_name(name)?;
        let c = self.db.lock().unwrap();
        if Self::name_taken(&c, &name, Some(id))? {
            return Err(AuthError::Taken("That player name is taken.".into()));
        }
        c.execute("UPDATE users SET name = ?1 WHERE id = ?2", params![name, id])?;
        Self::user_by_id(&c, id)?.ok_or_else(|| AuthError::Internal("no such user".into()))
    }

    /// Returns the raw token to put in the cookie.
    pub fn create_session(&self, user_id: i64) -> Result<String, AuthError> {
        let token = random_hex(32);
        let c = self.db.lock().unwrap();
        c.execute("DELETE FROM sessions WHERE expires < ?1", [now()])?;
        c.execute("INSERT INTO sessions (token_hash, user_id, expires) VALUES (?1, ?2, ?3)", params![token_hash(&token), user_id, now() + SESSION_DAYS * 86400])?;
        Ok(token)
    }

    pub fn session_user(&self, token: &str) -> Option<User> {
        if token.len() != 64 {
            return None;
        }
        let c = self.db.lock().unwrap();
        let id: Option<i64> = c.query_row("SELECT user_id FROM sessions WHERE token_hash = ?1 AND expires > ?2", params![token_hash(token), now()], |r| r.get(0)).optional().ok().flatten();
        id.and_then(|id| Self::user_by_id(&c, id).ok().flatten())
    }

    pub fn delete_session(&self, token: &str) {
        let c = self.db.lock().unwrap();
        let _ = c.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash(token)]);
    }

    /// Start a password reset for the account with this email. Returns the
    /// user and the raw token for the link, or None if no account uses the
    /// email. Only the newest link works; requesting another cancels it.
    pub fn create_reset(&self, email: &str) -> Result<Option<(User, String)>, AuthError> {
        let email = email.trim().to_lowercase();
        let c = self.db.lock().unwrap();
        let id: Option<i64> = c.query_row("SELECT id FROM users WHERE email = ?1", [&email], |r| r.get(0)).optional()?;
        let Some(id) = id else { return Ok(None) };
        let token = random_hex(32);
        c.execute("DELETE FROM reset_tokens WHERE user_id = ?1 OR expires < ?2", params![id, now()])?;
        c.execute("INSERT INTO reset_tokens (token_hash, user_id, expires) VALUES (?1, ?2, ?3)", params![token_hash(&token), id, now() + RESET_MINUTES * 60])?;
        Ok(Self::user_by_id(&c, id)?.map(|u| (u, token)))
    }

    /// Set a new password with a reset token. The token is used up, and every
    /// existing session of the account is signed out. Hashing is slow; call
    /// from a blocking task.
    pub fn reset_password(&self, token: &str, password: &str) -> Result<User, AuthError> {
        check_password(password)?;
        let invalid = || AuthError::Invalid("This reset link is invalid or has expired. Request a new one.".into());
        if token.len() != 64 {
            return Err(invalid());
        }
        let hash = hash_password(password)?;
        let c = self.db.lock().unwrap();
        let id: Option<i64> = c.query_row("SELECT user_id FROM reset_tokens WHERE token_hash = ?1 AND expires > ?2", params![token_hash(token), now()], |r| r.get(0)).optional()?;
        let Some(id) = id else { return Err(invalid()) };
        c.execute("UPDATE users SET pw_hash = ?1 WHERE id = ?2", params![hash, id])?;
        c.execute("DELETE FROM reset_tokens WHERE user_id = ?1", [id])?;
        c.execute("DELETE FROM sessions WHERE user_id = ?1", [id])?;
        Self::user_by_id(&c, id)?.ok_or_else(invalid)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_up_and_sign_in() {
        let a = Accounts::in_memory();
        let u = a.sign_up("Alice", " Alice@Example.com ", "correct horse battery").unwrap();
        assert_eq!(u.name, "Alice");
        assert_eq!(u.email.as_deref(), Some("alice@example.com"));
        assert!(u.has_password);
        assert_eq!(a.sign_in("ALICE@example.com", "correct horse battery").unwrap().id, u.id);
        assert_eq!(a.sign_in("alice@example.com", "wrong password!!"), Err(AuthError::BadCredentials));
        assert_eq!(a.sign_in("nobody@example.com", "correct horse battery"), Err(AuthError::BadCredentials));
    }

    #[test]
    fn validation_and_uniqueness() {
        let a = Accounts::in_memory();
        assert!(matches!(a.sign_up("Al", "a@b.co", "123456789012"), Err(AuthError::Invalid(_))));
        assert!(matches!(a.sign_up("Alice", "not-an-email", "123456789012"), Err(AuthError::Invalid(_))));
        assert!(matches!(a.sign_up("Alice", "a@b.co", "short"), Err(AuthError::Invalid(_))));
        assert!(matches!(a.sign_up("<script>", "a@b.co", "123456789012"), Err(AuthError::Invalid(_))));
        a.sign_up("Alice", "a@b.co", "123456789012").unwrap();
        assert!(matches!(a.sign_up("alice", "c@d.co", "123456789012"), Err(AuthError::Taken(_))));
        assert!(matches!(a.sign_up("Bob", "A@B.CO", "123456789012"), Err(AuthError::Taken(_))));
        let b = a.sign_up("Bob", "bob@b.co", "123456789012").unwrap();
        assert!(matches!(a.rename(b.id, "ALICE"), Err(AuthError::Taken(_))));
        assert_eq!(a.rename(b.id, "  Bobby   Tables ").unwrap().name, "Bobby Tables");
        assert_eq!(a.rename(b.id, "bobby tables").unwrap().name, "bobby tables");
    }

    #[test]
    fn sessions() {
        let a = Accounts::in_memory();
        let u = a.sign_up("Alice", "a@b.co", "123456789012").unwrap();
        let t = a.create_session(u.id).unwrap();
        assert_eq!(a.session_user(&t).unwrap().id, u.id);
        assert!(a.session_user(&random_hex(32)).is_none());
        assert!(a.session_user("short").is_none());
        a.delete_session(&t);
        assert!(a.session_user(&t).is_none());
    }

    #[test]
    fn password_reset() {
        let a = Accounts::in_memory();
        let u = a.sign_up("Alice", "alice@x.co", "old password 123").unwrap();
        let s = a.create_session(u.id).unwrap();
        assert!(a.create_reset("nobody@x.co").unwrap().is_none());
        let (who, t1) = a.create_reset(" ALICE@x.co ").unwrap().unwrap();
        assert_eq!(who.id, u.id);
        let (_, t2) = a.create_reset("alice@x.co").unwrap().unwrap();
        // An older link stops working once a newer one is sent.
        assert!(matches!(a.reset_password(&t1, "new password 456"), Err(AuthError::Invalid(_))));
        // The password rules still apply, and a rejected password keeps the link usable.
        assert!(matches!(a.reset_password(&t2, "short"), Err(AuthError::Invalid(_))));
        assert_eq!(a.reset_password(&t2, "new password 456").unwrap().id, u.id);
        assert_eq!(a.sign_in("alice@x.co", "old password 123"), Err(AuthError::BadCredentials));
        assert_eq!(a.sign_in("alice@x.co", "new password 456").unwrap().id, u.id);
        // Single use, and old sessions are signed out.
        assert!(matches!(a.reset_password(&t2, "another one 789"), Err(AuthError::Invalid(_))));
        assert!(a.session_user(&s).is_none());
        assert!(matches!(a.reset_password("garbage", "another one 789"), Err(AuthError::Invalid(_))));
    }

    #[test]
    fn oauth_links_and_creates() {
        let a = Accounts::in_memory();
        let alice = a.sign_up("Alice", "alice@x.co", "123456789012").unwrap();
        // Verified email links to the existing account.
        let g = a.oauth_login("google", "g-1", Some("alice@x.co"), true, "Alice G").unwrap();
        assert_eq!(g.id, alice.id);
        assert_eq!(g.providers, vec!["google"]);
        // Same identity again returns the same account.
        assert_eq!(a.oauth_login("google", "g-1", None, false, "whatever").unwrap().id, alice.id);
        // Unverified email does not take over the account and does not steal the email.
        let d = a.oauth_login("discord", "d-1", Some("alice@x.co"), false, "Alice").unwrap();
        assert_ne!(d.id, alice.id);
        assert_eq!(d.name, "Alice2");
        assert_eq!(d.email, None);
        assert!(!d.has_password);
        // Unusable display names fall back to a provider name.
        assert_eq!(a.oauth_login("twitch", "t-1", None, false, "!!").unwrap().name, "TwitchPlayer");
    }
}
