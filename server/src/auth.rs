//! Account HTTP API and OAuth sign-in.
//!
//!   GET  /api/me                      current user (or null) and enabled OAuth providers
//!   POST /api/sign-up                 {name, email, password}
//!   POST /api/sign-in                 {email, password}
//!   POST /api/sign-out
//!   POST /api/account                 {name}
//!   POST /api/forgot-password         {email}  emails a reset link
//!   POST /api/reset-password          {token, password}
//!   GET  /auth/{provider}/start       redirect to Google / Discord / Twitch
//!   GET  /auth/{provider}/callback    OAuth redirect target
//!
//! Sessions live in an HttpOnly, SameSite=Lax cookie. POSTs must be JSON and,
//! when the browser sends an Origin header, same-origin.

use crate::accounts::{AuthError, User};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const SESSION_COOKIE: &str = "rift_session";
const OAUTH_COOKIE: &str = "rift_oauth";
const OAUTH_TTL: Duration = Duration::from_secs(600);
/// Failed sign-ins allowed per email in `FAIL_WINDOW`.
const MAX_FAILS: usize = 8;
const FAIL_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Reset emails allowed per address in `FAIL_WINDOW`.
const MAX_RESETS: usize = 3;

#[derive(Clone, Debug)]
pub struct OAuthProvider {
    /// "google", "discord" or "twitch"; selects how the profile is read.
    pub id: String,
    pub label: String,
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scope: String,
}

#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    /// Public base URL, e.g. `https://rift.example.com`. Needed for OAuth redirect URIs.
    pub public_url: Option<String>,
    pub secure_cookies: bool,
    pub providers: Vec<OAuthProvider>,
}

impl AuthConfig {
    /// Reads `RIFT_PUBLIC_URL`, `RIFT_SECURE_COOKIES` and
    /// `RIFT_{GOOGLE,DISCORD,TWITCH}_CLIENT_ID` / `_CLIENT_SECRET`.
    pub fn from_env() -> AuthConfig {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let public_url = var("RIFT_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string());
        let secure_cookies = var("RIFT_SECURE_COOKIES").map(|v| v == "1" || v == "true").unwrap_or_else(|| public_url.as_deref().map(|u| u.starts_with("https://")).unwrap_or(false));
        let mut providers = vec![];
        for (id, label, auth, token, info, scope) in [
            ("google", "Google", "https://accounts.google.com/o/oauth2/v2/auth", "https://oauth2.googleapis.com/token", "https://openidconnect.googleapis.com/v1/userinfo", "openid email profile"),
            ("discord", "Discord", "https://discord.com/oauth2/authorize", "https://discord.com/api/oauth2/token", "https://discord.com/api/users/@me", "identify email"),
            ("twitch", "Twitch", "https://id.twitch.tv/oauth2/authorize", "https://id.twitch.tv/oauth2/token", "https://api.twitch.tv/helix/users", "user:read:email"),
        ] {
            let up = id.to_uppercase();
            if let (Some(cid), Some(secret)) = (var(&format!("RIFT_{up}_CLIENT_ID")), var(&format!("RIFT_{up}_CLIENT_SECRET"))) {
                if public_url.is_none() {
                    tracing::warn!("{label} sign-in is configured but RIFT_PUBLIC_URL is not set; it stays disabled");
                    continue;
                }
                providers.push(OAuthProvider { id: id.into(), label: label.into(), client_id: cid, client_secret: secret, auth_url: auth.into(), token_url: token.into(), userinfo_url: info.into(), scope: scope.into() });
            }
        }
        AuthConfig { public_url, secure_cookies, providers }
    }

    fn provider(&self, id: &str) -> Option<&OAuthProvider> {
        self.providers.iter().find(|p| p.id == id)
    }

    fn redirect_uri(&self, p: &OAuthProvider) -> String {
        format!("{}/auth/{}/callback", self.public_url.as_deref().unwrap_or(""), p.id)
    }
}

/// Short-lived server-side state for OAuth round trips and sign-in throttling.
#[derive(Default)]
pub struct AuthRuntime {
    pending: HashMap<String, Pending>,
    fails: HashMap<String, Vec<Instant>>,
    resets: HashMap<String, Vec<Instant>>,
}

struct Pending {
    provider: String,
    verifier: String,
    redirect: String,
    created: Instant,
}

impl AuthRuntime {
    fn throttled(&mut self, email: &str) -> bool {
        let now = Instant::now();
        let v = self.fails.entry(email.to_string()).or_default();
        v.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
        v.len() >= MAX_FAILS
    }
    fn record_fail(&mut self, email: &str) {
        self.fails.entry(email.to_string()).or_default().push(Instant::now());
    }
    fn clear_fails(&mut self, email: &str) {
        self.fails.remove(email);
    }
    /// Counts a reset request; false if this address asked too often.
    fn allow_reset(&mut self, email: &str) -> bool {
        let now = Instant::now();
        let v = self.resets.entry(email.to_string()).or_default();
        v.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
        if v.len() >= MAX_RESETS {
            return false;
        }
        v.push(now);
        true
    }
    fn sweep(&mut self) {
        let now = Instant::now();
        self.resets.retain(|_, v| v.iter().any(|t| now.duration_since(*t) < FAIL_WINDOW));
        self.pending.retain(|_, p| now.duration_since(p.created) < OAUTH_TTL);
        self.fails.retain(|_, v| v.iter().any(|t| now.duration_since(*t) < FAIL_WINDOW));
    }
}

// ------------------------------------------------------------------ helpers

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// True unless the request carries an Origin header for a different host.
pub fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

pub fn session_user(s: &AppState, headers: &HeaderMap) -> Option<User> {
    cookie(headers, SESSION_COOKIE).and_then(|t| s.accounts.session_user(&t))
}

fn set_cookie(cfg: &AuthConfig, name: &str, value: &str, path: &str, max_age: i64) -> HeaderValue {
    let secure = if cfg.secure_cookies { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")).expect("ascii cookie")
}

fn session_cookie(cfg: &AuthConfig, token: &str) -> HeaderValue {
    set_cookie(cfg, SESSION_COOKIE, token, "/", crate::accounts::SESSION_DAYS * 86400)
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn auth_err(e: AuthError) -> Response {
    let status = match &e {
        AuthError::Invalid(_) => StatusCode::BAD_REQUEST,
        AuthError::Taken(_) => StatusCode::CONFLICT,
        AuthError::BadCredentials => StatusCode::UNAUTHORIZED,
        AuthError::Internal(m) => {
            tracing::error!("auth: {m}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    err(status, e.message())
}

/// Rejects cross-origin and non-JSON POSTs.
fn check_post(headers: &HeaderMap) -> Result<(), Response> {
    let json = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|v| v.starts_with("application/json")).unwrap_or(false);
    if !json || !same_origin(headers) {
        return Err(err(StatusCode::FORBIDDEN, "Request blocked."));
    }
    Ok(())
}

fn signed_in(s: &AppState, user: &User) -> Response {
    let token = match s.accounts.create_session(user.id) {
        Ok(t) => t,
        Err(e) => return auth_err(e),
    };
    let mut r = Json(json!({ "user": user })).into_response();
    r.headers_mut().insert(header::SET_COOKIE, session_cookie(&s.auth, &token));
    r
}

/// Only same-site paths; anything else falls back to "/".
fn local_redirect(r: Option<&str>) -> String {
    match r {
        Some(p) if p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') && !p.chars().any(|c| c.is_control()) => p.to_string(),
        _ => "/".into(),
    }
}

fn redirect(to: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, to.to_string())]).into_response()
}

fn with_query(path: &str, k: &str, v: &str) -> String {
    let q = serde_urlencoded::to_string([(k, v)]).unwrap_or_default();
    format!("{path}{}{q}", if path.contains('?') { '&' } else { '?' })
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

// ------------------------------------------------------------------ handlers

pub async fn me(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let providers: Vec<_> = s.auth.providers.iter().map(|p| json!({ "id": p.id, "label": p.label })).collect();
    let user = session_user(&s, &headers);
    ([(header::CACHE_CONTROL, "no-store")], Json(json!({ "user": user, "providers": providers, "min_password": crate::accounts::MIN_PASSWORD }))).into_response()
}

#[derive(Deserialize)]
pub struct SignUp {
    name: String,
    email: String,
    password: String,
}

pub async fn sign_up(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<SignUp>) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    let acc = s.accounts.clone();
    match tokio::task::spawn_blocking(move || acc.sign_up(&b.name, &b.email, &b.password)).await {
        Ok(Ok(u)) => signed_in(&s, &u),
        Ok(Err(e)) => auth_err(e),
        Err(e) => auth_err(AuthError::Internal(e.to_string())),
    }
}

#[derive(Deserialize)]
pub struct SignIn {
    email: String,
    password: String,
}

pub async fn sign_in(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<SignIn>) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    let email = b.email.trim().to_lowercase();
    if s.auth_rt.lock().unwrap().throttled(&email) {
        return err(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Wait a few minutes and try again.");
    }
    let acc = s.accounts.clone();
    let e2 = email.clone();
    match tokio::task::spawn_blocking(move || acc.sign_in(&e2, &b.password)).await {
        Ok(Ok(u)) => {
            s.auth_rt.lock().unwrap().clear_fails(&email);
            signed_in(&s, &u)
        }
        Ok(Err(e)) => {
            if e == AuthError::BadCredentials {
                s.auth_rt.lock().unwrap().record_fail(&email);
            }
            auth_err(e)
        }
        Err(e) => auth_err(AuthError::Internal(e.to_string())),
    }
}

pub async fn sign_out(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    if let Some(t) = cookie(&headers, SESSION_COOKIE) {
        s.accounts.delete_session(&t);
    }
    let mut r = Json(json!({ "user": null })).into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&s.auth, SESSION_COOKIE, "", "/", 0));
    r
}

#[derive(Deserialize)]
pub struct AccountUpdate {
    name: String,
}

pub async fn update_account(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<AccountUpdate>) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    let Some(user) = session_user(&s, &headers) else { return err(StatusCode::UNAUTHORIZED, "Sign in first.") };
    match s.accounts.rename(user.id, &b.name) {
        Ok(u) => {
            s.hub.lock().unwrap().rename_user(u.id, &u.name);
            Json(json!({ "user": u })).into_response()
        }
        Err(e) => auth_err(e),
    }
}

#[derive(Deserialize)]
pub struct Forgot {
    email: String,
}

/// Always answers the same way, whether or not an account uses the email, and
/// does the lookup and sending in the background so timing doesn't tell either.
pub async fn forgot_password(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<Forgot>) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    let email = match crate::accounts::check_email(&b.email) {
        Ok(e) => e,
        Err(e) => return auth_err(e),
    };
    if !s.auth_rt.lock().unwrap().allow_reset(&email) {
        return err(StatusCode::TOO_MANY_REQUESTS, "Too many reset emails for this address. Wait a few minutes and try again.");
    }
    // Links use the configured public URL. Only the log mailer (local testing)
    // falls back to the Host header, since nobody else sees those links.
    let base = match (&s.auth.public_url, s.mailer.delivers()) {
        (Some(u), _) => Some(u.clone()),
        (None, false) => headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(|h| format!("http://{h}")),
        (None, true) => None,
    };
    tokio::spawn(async move {
        let Some(base) = base else {
            tracing::error!("password reset requested but RIFT_PUBLIC_URL is not set");
            return;
        };
        let (user, token) = match s.accounts.create_reset(&email) {
            Ok(Some(x)) => x,
            Ok(None) => return,
            Err(e) => {
                tracing::error!("password reset: {}", e.message());
                return;
            }
        };
        let link = format!("{base}/?reset={token}");
        let mail = crate::mail::Mail {
            to: email.clone(),
            subject: "Reset your Rift Showdown password".into(),
            body: format!(
                "Hi {},\n\nSomeone asked to reset the password for your Rift Showdown account. To choose a new password, open this link within {} minutes:\n\n{link}\n\nIf you didn't ask for this, ignore this email. Your password stays the same.\n",
                user.name,
                crate::accounts::RESET_MINUTES
            ),
        };
        if let Err(e) = s.mailer.send(mail).await {
            tracing::error!("sending password reset email: {e}");
        }
    });
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub struct Reset {
    token: String,
    password: String,
}

pub async fn reset_password(State(s): State<AppState>, headers: HeaderMap, Json(b): Json<Reset>) -> Response {
    if let Err(r) = check_post(&headers) {
        return r;
    }
    let acc = s.accounts.clone();
    match tokio::task::spawn_blocking(move || acc.reset_password(&b.token, &b.password)).await {
        Ok(Ok(u)) => {
            if let Some(e) = &u.email {
                s.auth_rt.lock().unwrap().clear_fails(e);
            }
            // Other devices lose their sessions; tell any open socket now.
            s.hub.lock().unwrap().sign_out_user(u.id);
            signed_in(&s, &u)
        }
        Ok(Err(e)) => auth_err(e),
        Err(e) => auth_err(AuthError::Internal(e.to_string())),
    }
}

pub async fn oauth_start(State(s): State<AppState>, Path(provider): Path<String>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(p) = s.auth.provider(&provider) else { return err(StatusCode::NOT_FOUND, "That sign-in method is not enabled.") };
    let state = crate::accounts::random_hex(16);
    let verifier = crate::accounts::random_hex(32);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let redirect_to = local_redirect(q.get("redirect").map(|s| s.as_str()));
    {
        let mut rt = s.auth_rt.lock().unwrap();
        rt.sweep();
        rt.pending.insert(state.clone(), Pending { provider: p.id.clone(), verifier, redirect: redirect_to, created: Instant::now() });
    }
    let query = serde_urlencoded::to_string([
        ("response_type", "code"),
        ("client_id", p.client_id.as_str()),
        ("redirect_uri", s.auth.redirect_uri(p).as_str()),
        ("scope", p.scope.as_str()),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ])
    .unwrap_or_default();
    let mut r = redirect(&format!("{}?{query}", p.auth_url));
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&s.auth, OAUTH_COOKIE, &state, "/auth", OAUTH_TTL.as_secs() as i64));
    r
}

/// Profile fields we need from a provider.
#[derive(Debug, PartialEq)]
pub struct Profile {
    pub subject: String,
    pub name: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

pub fn parse_profile(provider: &str, v: &serde_json::Value) -> Option<Profile> {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    match provider {
        "discord" => Some(Profile {
            subject: s(v, "id")?,
            name: s(v, "global_name").or_else(|| s(v, "username")).unwrap_or_default(),
            email: s(v, "email"),
            email_verified: v.get("verified").and_then(|x| x.as_bool()).unwrap_or(false),
        }),
        "twitch" => {
            let u = v.get("data")?.get(0)?;
            // Twitch only returns verified email addresses.
            Some(Profile { subject: s(u, "id")?, name: s(u, "display_name").or_else(|| s(u, "login")).unwrap_or_default(), email: s(u, "email"), email_verified: s(u, "email").is_some() })
        }
        _ => Some(Profile {
            subject: s(v, "sub")?,
            name: s(v, "name").or_else(|| s(v, "given_name")).unwrap_or_default(),
            email: s(v, "email"),
            email_verified: v.get("email_verified").and_then(|x| x.as_bool()).unwrap_or(false),
        }),
    }
}

async fn fetch_profile(s: &AppState, p: &OAuthProvider, code: &str, verifier: &str) -> Result<Profile, String> {
    let form = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", s.auth.redirect_uri(p).as_str()),
        ("client_id", p.client_id.as_str()),
        ("client_secret", p.client_secret.as_str()),
        ("code_verifier", verifier),
    ])
    .map_err(|e| e.to_string())?;
    let r = s.http.post(&p.token_url).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").header(header::ACCEPT, "application/json").body(form).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("token endpoint returned {}", r.status()));
    }
    let tok: serde_json::Value = serde_json::from_str(&r.text().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let access = tok.get("access_token").and_then(|v| v.as_str()).ok_or("no access_token")?;
    let r = s.http.get(&p.userinfo_url).bearer_auth(access).header("Client-Id", &p.client_id).header(header::ACCEPT, "application/json").send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("profile endpoint returned {}", r.status()));
    }
    let v: serde_json::Value = serde_json::from_str(&r.text().await.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    parse_profile(&p.id, &v).ok_or_else(|| "profile without an id".into())
}

pub async fn oauth_callback(State(s): State<AppState>, Path(provider): Path<String>, headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    let state = q.get("state").cloned().unwrap_or_default();
    let pending = s.auth_rt.lock().unwrap().pending.remove(&state);
    let clear = set_cookie(&s.auth, OAUTH_COOKIE, "", "/auth", 0);
    let fail = |to: &str, msg: &str| {
        let mut r = redirect(&with_query(to, "auth_error", msg));
        r.headers_mut().insert(header::SET_COOKIE, clear.clone());
        r
    };
    let Some(pending) = pending.filter(|p| p.provider == provider && p.created.elapsed() < OAUTH_TTL && cookie(&headers, OAUTH_COOKIE).as_deref() == Some(state.as_str())) else {
        return fail("/", "Sign-in expired. Try again.");
    };
    let Some(p) = s.auth.provider(&provider).cloned() else { return fail(&pending.redirect, "That sign-in method is not enabled.") };
    if q.contains_key("error") {
        return fail(&pending.redirect, "Sign-in was cancelled.");
    }
    let Some(code) = q.get("code") else { return fail(&pending.redirect, "Sign-in failed. Try again.") };
    let profile = match fetch_profile(&s, &p, code, &pending.verifier).await {
        Ok(pr) => pr,
        Err(e) => {
            tracing::warn!("{} sign-in failed: {e}", p.label);
            return fail(&pending.redirect, &format!("{} sign-in failed. Try again.", p.label));
        }
    };
    let user = match s.accounts.oauth_login(&p.id, &profile.subject, profile.email.as_deref(), profile.email_verified, &profile.name) {
        Ok(u) => u,
        Err(e) => return fail(&pending.redirect, &e.message()),
    };
    let token = match s.accounts.create_session(user.id) {
        Ok(t) => t,
        Err(e) => return fail(&pending.redirect, &e.message()),
    };
    let mut r = redirect(&pending.redirect);
    r.headers_mut().append(header::SET_COOKIE, session_cookie(&s.auth, &token));
    r.headers_mut().append(header::SET_COOKIE, clear);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(b64url(b"\xff\xff\xff"), "____");
        assert_eq!(b64url(&Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(local_redirect(Some("/?room=ABC")), "/?room=ABC");
        assert_eq!(local_redirect(Some("//evil.com")), "/");
        assert_eq!(local_redirect(Some("https://evil.com")), "/");
        assert_eq!(local_redirect(Some("/\\evil.com")), "/");
        assert_eq!(local_redirect(None), "/");
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; rift_session=abc; b=2"));
        assert_eq!(cookie(&h, "rift_session").as_deref(), Some("abc"));
        h.insert(header::HOST, HeaderValue::from_static("localhost:8080"));
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, HeaderValue::from_static("http://localhost:8080"));
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&h));
    }

    #[test]
    fn profiles() {
        let g = parse_profile("google", &json!({"sub":"1","name":"Ann","email":"a@b.co","email_verified":true})).unwrap();
        assert_eq!(g, Profile { subject: "1".into(), name: "Ann".into(), email: Some("a@b.co".into()), email_verified: true });
        let d = parse_profile("discord", &json!({"id":"9","username":"ann_1","global_name":null,"email":"a@b.co","verified":false})).unwrap();
        assert_eq!((d.name.as_str(), d.email_verified), ("ann_1", false));
        let t = parse_profile("twitch", &json!({"data":[{"id":"7","login":"ann","display_name":"Ann","email":"a@b.co"}]})).unwrap();
        assert_eq!((t.subject.as_str(), t.name.as_str(), t.email_verified), ("7", "Ann", true));
        assert!(parse_profile("twitch", &json!({"data":[]})).is_none());
    }
}
