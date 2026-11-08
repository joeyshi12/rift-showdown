//! Outgoing email (password reset links).
//!
//! `RIFT_SMTP_URL` selects an SMTP server, e.g.
//! `smtps://user:pass@smtp.example.com` (TLS, port 465) or
//! `smtp://user:pass@smtp.example.com:587?tls=required` (STARTTLS).
//! `RIFT_MAIL_FROM` is the sender, e.g. `Rift Online <noreply@example.com>`.
//! Without `RIFT_SMTP_URL` the server writes messages to its log, which is
//! enough for local testing but means nobody else receives them.

use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[derive(Clone)]
pub enum Mailer {
    Smtp { transport: AsyncSmtpTransport<Tokio1Executor>, from: Mailbox },
    /// Development: write the message to the server log.
    Log,
    /// Tests: keep messages in memory.
    Memory(Arc<Mutex<Vec<Mail>>>),
}

impl Mailer {
    pub fn from_env() -> Result<Mailer, String> {
        let Some(url) = std::env::var("RIFT_SMTP_URL").ok().filter(|v| !v.trim().is_empty()) else {
            tracing::warn!("RIFT_SMTP_URL is not set: password reset links are written to the server log instead of being emailed");
            return Ok(Mailer::Log);
        };
        let from = std::env::var("RIFT_MAIL_FROM").map_err(|_| "RIFT_SMTP_URL is set but RIFT_MAIL_FROM is not".to_string())?;
        let from: Mailbox = from.parse().map_err(|e| format!("RIFT_MAIL_FROM: {e}"))?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&url).map_err(|e| format!("RIFT_SMTP_URL: {e}"))?.build();
        Ok(Mailer::Smtp { transport, from })
    }

    /// True when mail actually leaves the server.
    pub fn delivers(&self) -> bool {
        matches!(self, Mailer::Smtp { .. })
    }

    pub async fn send(&self, mail: Mail) -> Result<(), String> {
        match self {
            Mailer::Smtp { transport, from } => {
                let to: Mailbox = mail.to.parse().map_err(|e| format!("bad recipient: {e}"))?;
                let msg = Message::builder().from(from.clone()).to(to).subject(&mail.subject).body(mail.body).map_err(|e| e.to_string())?;
                transport.send(msg).await.map(|_| ()).map_err(|e| e.to_string())
            }
            Mailer::Log => {
                tracing::info!("email to {}: {}\n{}", mail.to, mail.subject, mail.body);
                Ok(())
            }
            Mailer::Memory(v) => {
                v.lock().unwrap().push(mail);
                Ok(())
            }
        }
    }
}
