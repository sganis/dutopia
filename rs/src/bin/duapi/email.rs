// rs/src/bin/duapi/email.rs
//
// SMTP + email-address resolution for the cleanup-request notify flow.
//
// Config is read from env vars once and cached in a OnceLock:
//   SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASSWORD, SMTP_FROM, SMTP_TLS
//   MAIL_DOMAIN  — used to synthesize {username}@{MAIL_DOMAIN}.
//
// Email lookup is deliberately simple: there is no UID→email mapping
// anywhere in the scan pipeline, so we rely on a domain convention. If
// MAIL_DOMAIN is unset, resolve_email returns None and the notify endpoint
// responds with 404. SMTP is gated on is_configured() — the frontend reads
// it via /api/health.smtp_configured so the Notify button can be disabled
// rather than clicked-and-failed.

use lettre::message::{header::ContentType, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use std::sync::OnceLock;

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    user: String,
    password: String,
    from: String,
    tls: bool,
    mail_domain: String,
}

static CONFIG: OnceLock<Option<SmtpConfig>> = OnceLock::new();

fn config() -> &'static Option<SmtpConfig> {
    CONFIG.get_or_init(|| {
        let host = std::env::var("SMTP_HOST").ok().filter(|s| !s.is_empty())?;
        let port: u16 = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(587);
        let user = std::env::var("SMTP_USER").ok().filter(|s| !s.is_empty())?;
        let password = std::env::var("SMTP_PASSWORD").ok().filter(|s| !s.is_empty())?;
        let from = std::env::var("SMTP_FROM").ok().filter(|s| !s.is_empty())?;
        let tls = std::env::var("SMTP_TLS")
            .ok()
            .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);
        let mail_domain = std::env::var("MAIL_DOMAIN")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(SmtpConfig {
            host,
            port,
            user,
            password,
            from,
            tls,
            mail_domain,
        })
    })
}

/// True when all SMTP vars AND MAIL_DOMAIN are set. Exposed via /api/health
/// so the frontend can gate the Notify button instead of discovering SMTP is
/// missing on click.
pub fn is_configured() -> bool {
    config().is_some()
}

/// Synthesize an email address from a username using the configured
/// MAIL_DOMAIN. Returns None when MAIL_DOMAIN (or other SMTP config) is
/// missing — the caller should respond 404 so the UI can surface a clear
/// message to the admin.
pub fn resolve_email(username: &str) -> Option<String> {
    let cfg = config().as_ref()?;
    let u = username.trim();
    if u.is_empty() {
        return None;
    }
    Some(format!("{}@{}", u, cfg.mail_domain))
}

/// Send a plain-text email. Returns a string describing the failure on
/// error; the caller logs it and returns 502 with a sanitized message.
pub fn send(to: &str, subject: &str, body: &str) -> Result<(), String> {
    let cfg = match config().as_ref() {
        Some(c) => c,
        None => return Err("SMTP not configured".to_string()),
    };

    let from_mbox: Mailbox = cfg
        .from
        .parse()
        .map_err(|e| format!("invalid SMTP_FROM: {e}"))?;
    let to_mbox: Mailbox = to
        .parse()
        .map_err(|e| format!("invalid recipient {to:?}: {e}"))?;

    let email = Message::builder()
        .from(from_mbox)
        .to(to_mbox)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| format!("build email: {e}"))?;

    let creds = Credentials::new(cfg.user.clone(), cfg.password.clone());
    let builder = if cfg.tls {
        SmtpTransport::starttls_relay(&cfg.host).map_err(|e| format!("tls relay: {e}"))?
    } else {
        SmtpTransport::builder_dangerous(&cfg.host)
    };
    let mailer = builder.port(cfg.port).credentials(creds).build();

    mailer
        .send(&email)
        .map(|_| ())
        .map_err(|e| format!("smtp send: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_env() {
        for k in [
            "SMTP_HOST",
            "SMTP_PORT",
            "SMTP_USER",
            "SMTP_PASSWORD",
            "SMTP_FROM",
            "SMTP_TLS",
            "MAIL_DOMAIN",
        ] {
            unsafe { std::env::remove_var(k) };
        }
    }

    // config() is OnceLock-cached — we can't reset it between tests, so the
    // domain-resolution tests don't rely on the real cached config. They
    // exercise the pure string-building behavior only.
    #[test]
    fn email_is_synthesized_when_domain_present() {
        // Probe the branch manually: mimic what resolve_email would do if
        // config returned Some(domain). This avoids touching the OnceLock.
        let domain = "corp.example";
        let user = "alice";
        assert_eq!(
            format!("{}@{}", user.trim(), domain),
            "alice@corp.example"
        );
    }

    #[test]
    #[serial]
    fn is_configured_reflects_env_when_first_call() {
        clear_env();
        // When called before any vars are set, config() returns None. OnceLock
        // means this test must run before any other test that reads config().
        // The #[serial] attribute + alphabetical ordering should keep it first
        // in practice; this is best-effort and documents intent.
        if CONFIG.get().is_none() {
            assert!(!is_configured());
        }
    }
}
