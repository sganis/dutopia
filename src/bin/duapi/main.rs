// rs/src/bin/duapi/main.rs
use anyhow::{Context, Result};
use axum::{
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use clap::{ColorChoice, Parser};
use colored::Colorize;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;

use dutopia::db;
use dutopia::util::logging::init_tracing;
use dutopia::util::print_about;

mod cleanup;
mod email;
mod handler;
mod mcp;
mod oidc;
mod query;
mod shutdown;

use db::DbPool;
use handler::{get_files_handler, get_folders_handler, health_handler, login_handler, users_handler};

static DB_POOL: OnceLock<DbPool> = OnceLock::new();
static USERS: OnceLock<Vec<String>> = OnceLock::new();
/// Whether the DB carries per-file rows (`dudb --raw`); probed once at
/// startup. When false, /api/files falls back to the live filesystem.
static FILES_INDEXED: OnceLock<bool> = OnceLock::new();

#[cfg(test)]
static TEST_DB: OnceLock<db::test_support::TempDb> = OnceLock::new();

/// Environment variable reference shown by `-h`/`--help`. Keep in sync with
/// the vars read in this file, oidc.rs, email.rs, query.rs and src/auth.rs.
const ENV_HELP: &str = r#"Environment variables (also read from a .env file in the working directory):

Required:
  JWT_SECRET              Secret used to sign the internal session JWTs.
                          duapi refuses to start without it.

Server (CLI flags take precedence over these):
  DB_PATH                 SQLite database path (same as the positional argument)
  STATIC_DIR              UI folder (same as --static-dir)
  PORT                    Listen port (same as --port; default: 8080)
  TLS_CERT, TLS_KEY       PEM certificate and private key paths (same as
                          --tls-cert/--tls-key). Set BOTH to serve HTTPS;
                          omit both to serve plain HTTP. No other certificate
                          is required to run duapi.
  CORS_ORIGIN             Allowed CORS origin (same as --cors-origin),
                          e.g. http://localhost:5173

Authentication:
  Local login (POST /api/login) is always enabled and verifies credentials
  against the host's local accounts (Linux: su/PAM, macOS: dscl). It needs
  no certificates and no OIDC configuration.

  ADMIN_GROUP             Comma-separated names granted admin. Matches the
                          username on local logins; matches the username or
                          any Keycloak group/realm-role on OIDC logins.
  ADMIN_PASSWORD          Debug/test builds only: this password logs any
                          username in as admin. Compiled out of release
                          builds, where it is ignored.
  FAKE_USER,
  FAKE_PASSWORD           Windows only: credentials for the fake
                          authenticator (defaults: current user / "admin").

OIDC / Keycloak single sign-on (optional; enabled only when OIDC_ISSUER is set):
  OIDC_ISSUER             Issuer URL, e.g. https://kc.example.com/realms/main
  OIDC_CLIENT_ID          Client id (required when OIDC_ISSUER is set)
  OIDC_CLIENT_SECRET      Client secret (required when OIDC_ISSUER is set)
  OIDC_REDIRECT_URI       Callback URL (required when OIDC_ISSUER is set),
                          e.g. https://host:8443/api/auth/callback
  OIDC_SCOPES             Requested scopes (default: "openid profile email")
  OIDC_USERNAME_CLAIM     ID-token claim used as the username
                          (default: preferred_username)
  OIDC_POST_LOGIN_REDIRECT
                          Path to land on after login (default: /)
  OIDC_CA_CERT            Path to a PEM CA bundle trusted for backchannel
                          calls to the issuer. Only needed when Keycloak
                          serves a self-signed or private-CA certificate.
  OIDC_API_AUDIENCES      Comma-separated accepted `aud` values for RS256
                          bearer access tokens (default: OIDC_CLIENT_ID)

Request limits:
  REQUEST_TIMEOUT_SECS    Per-request timeout in seconds (default: 30)
  MAX_BODY_BYTES          Max request body size in bytes (default: 65536)
  MAX_PAGE_SIZE           Max rows per /api/files page (default: 2000)

Email notifications (POST /api/cleanup/notify; disabled unless all are set):
  SMTP_HOST, SMTP_USER, SMTP_PASSWORD, SMTP_FROM
  SMTP_PORT               SMTP port (default: 587)
  SMTP_TLS                Use STARTTLS: 1/true/yes/on (default: on)
  MAIL_DOMAIN             Recipient domain: mail is sent to
                          <username>@MAIL_DOMAIN"#;

#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Disk usage API server with web UI",
    after_help = ENV_HELP
)]
struct Args {
    /// Input SQLite database file path (built by `dudb`). Falls back to DB_PATH env var.
    #[arg(env = "DB_PATH")]
    input: Option<PathBuf>,
    /// UI folder (defaults to STATIC_DIR env var or local public directory)
    #[arg(short, long, value_name = "DIR", env = "STATIC_DIR")]
    static_dir: Option<String>,
    /// Port number (defaults to PORT env var or 8080)
    #[arg(short, long, env = "PORT")]
    port: Option<u16>,
    /// Enable HTTPS with certificate file path (falls back to TLS_CERT env var)
    #[arg(long, value_name = "FILE")]
    tls_cert: Option<PathBuf>,
    /// Private key file path (falls back to TLS_KEY env var; required if tls-cert is set)
    #[arg(long, value_name = "FILE")]
    tls_key: Option<PathBuf>,
    /// CORS allowed origin (falls back to CORS_ORIGIN env var)
    #[arg(long, value_name = "URL")]
    cors_origin: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    print_about();
    init_tracing("duapi");

    if let Ok(env_path) = dotenvy::dotenv()
        && let Some(dir) = env_path.parent()
    {
        oidc::set_env_dir(dir.to_path_buf());
    }
    if std::env::var("JWT_SECRET").is_err() {
        eprintln!(
            "{}",
            "FATAL: JWT_SECRET environment variable is required. Set it before starting duapi."
                .red()
        );
        std::process::exit(1);
    }

    let args = Args::parse();
    let db_path = match args.input.clone() {
        Some(p) => p,
        None => {
            eprintln!(
                "{}",
                "FATAL: database path required. Pass as argument or set DB_PATH env var.".red()
            );
            std::process::exit(1);
        }
    };
    let static_dir: String = args
        .static_dir
        .or_else(|| std::env::var("STATIC_DIR").ok())
        .unwrap_or_else(default_static_dir);
    let port = args
        .port
        .or_else(|| std::env::var("PORT").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(8080);

    if is_port_taken(port) {
        eprintln!(
            "{}",
            format!(
                "Error: Port {port} is already in use. Try another port with --port or PORT env var."
            )
            .red()
        );
        std::process::exit(1);
    }

    match std::env::var("ADMIN_GROUP") {
        Ok(g) => {
            println!("ADMIN_GROUP={g}");
        }
        Err(_) => {
            eprintln!("{}", "Warning: ADMIN_GROUP env var is not set.".yellow());
        }
    }

    if let Err(e) = oidc::init().await {
        eprintln!("{}", format!("FATAL: OIDC init failed: {e:#}").red());
        std::process::exit(1);
    }
    println!(
        "Auth mode: password{}",
        if oidc::is_enabled() { " + oidc" } else { "" }
    );

    println!("Opening database: {}", db_path.display());
    let pool = db::open_pool(&db_path).with_context(|| {
        format!(
            "opening DB at {}. Build it first with `dudb --input <csv> --output {}`",
            db_path.display(),
            db_path.display()
        )
    })?;
    let users = db::list_users(&pool).context("loading user list")?;
    println!("Loaded {} users", users.len());

    let files_indexed = db::has_files(&pool).unwrap_or(false);
    if files_indexed {
        println!("File index: present (/api/files served from DB)");
    } else {
        println!(
            "{}",
            "File index: absent (/api/files reads the live filesystem; \
             rebuild with `dudb --raw <scan.csv>` to serve from DB)"
                .yellow()
        );
    }
    let _ = FILES_INDEXED.set(files_indexed);

    if DB_POOL.set(pool).is_err() {
        eprintln!("{}", "FATAL: DB_POOL already initialized".red());
        std::process::exit(1);
    }
    if USERS.set(users).is_err() {
        eprintln!("{}", "FATAL: USERS already initialized".red());
        std::process::exit(1);
    }

    let cors_origin = args
        .cors_origin
        .or_else(|| std::env::var("CORS_ORIGIN").ok())
        .filter(|s| !s.trim().is_empty());
    let tls_cert = args
        .tls_cert
        .or_else(|| env_path_nonempty("TLS_CERT"));
    let tls_key = args
        .tls_key
        .or_else(|| env_path_nonempty("TLS_KEY"));

    let cors = if let Some(ref origin) = cors_origin {
        let header = match parse_cors_origin(origin) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("{}", format!("FATAL: {e}").red());
                std::process::exit(1);
            }
        };
        CorsLayer::new()
            .allow_origin(header)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(Any)
    } else {
        CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers(Any)
    };

    let api = Router::new()
        .route("/health", get(health_handler))
        .route("/login", post(login_handler))
        .route("/auth/mode", get(oidc::mode_handler))
        .route("/auth/login", get(oidc::login_handler))
        .route("/auth/callback", get(oidc::callback_handler))
        .route("/users", get(users_handler))
        .route("/folders", get(get_folders_handler))
        .route("/files", get(get_files_handler))
        .route("/mcp", post(mcp::handler))
        .route("/cleanup/script", post(cleanup::script_handler))
        .route("/cleanup/notify", post(cleanup::notify_handler))
        .fallback(api_not_found);

    let frontend = ServeDir::new(&static_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", static_dir)));

    let timeout_secs = env_u64("REQUEST_TIMEOUT_SECS", 30);
    let body_limit_bytes = env_u64("MAX_BODY_BYTES", 64 * 1024) as usize;
    tracing::info!(timeout_secs, body_limit_bytes, "request limits configured");

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
        .layer(cors)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(timeout_secs),
        ))
        .layer(RequestBodyLimitLayer::new(body_limit_bytes));

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            if !cert_path.exists() {
                eprintln!(
                    "{}",
                    format!("Error: Certificate file not found: {}", cert_path.display()).red()
                );
                std::process::exit(1);
            }
            if !key_path.exists() {
                eprintln!(
                    "{}",
                    format!("Error: Key file not found: {}", key_path.display()).red()
                );
                std::process::exit(1);
            }

            println!(
                "Loading TLS certificate from {} and key from {}",
                cert_path.display(),
                key_path.display()
            );

            let config = RustlsConfig::from_pem_file(cert_path, key_path)
                .await
                .context("Failed to load TLS certificate/key")?;

            println!("Serving on https://{addr}  (static dir: {static_dir})");
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            tokio::spawn(async move {
                shutdown::shutdown_signal().await;
                shutdown_handle.graceful_shutdown(Some(Duration::from_secs(30)));
            });

            axum_server::bind_rustls(addr, config)
                .handle(handle)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await?;
        }
        (None, None) => {
            println!("Serving on http://{addr}  (static dir: {static_dir})");
            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown::shutdown_signal())
            .await?;
        }
        _ => {
            eprintln!(
                "{}",
                "Error: Both --tls-cert and --tls-key must be provided together".red()
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

/// JSON 404 for unmatched `/api/*` paths so typos/wrong methods don't leak into
/// the SPA fallback and return `index.html`.
async fn api_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found" })),
    )
}

/// Parse a CORS origin string into the `HeaderValue` that `tower_http`'s `CorsLayer` requires.
fn parse_cors_origin(s: &str) -> Result<axum::http::HeaderValue, anyhow::Error> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        anyhow::bail!("CORS_ORIGIN is empty");
    }
    if trimmed.contains(char::is_whitespace) || !trimmed.contains("://") {
        anyhow::bail!("invalid CORS_ORIGIN value: {trimmed:?}");
    }
    trimmed
        .parse::<axum::http::HeaderValue>()
        .with_context(|| format!("invalid CORS_ORIGIN value: {trimmed:?}"))
}

fn env_path_nonempty(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

pub fn is_port_taken(port: u16) -> bool {
    let addrs = [format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
    for a in addrs {
        if TcpStream::connect_timeout(&a.parse().unwrap(), Duration::from_millis(120)).is_ok() {
            return true;
        }
    }
    false
}

fn default_static_dir() -> String {
    let mut exe_dir = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe_dir.pop();
    let static_dir = exe_dir.join("public");
    eprintln!(
        "{}",
        format!("Using default static dir: {}", static_dir.display()).yellow()
    );
    static_dir.to_string_lossy().into_owned()
}

pub fn get_db() -> &'static DbPool {
    DB_POOL.get().expect("DB pool not initialized")
}

pub fn get_users() -> &'static Vec<String> {
    USERS.get().expect("User list not initialized")
}

/// Unset (only possible in tests that skip DB init) means "no file index".
pub fn files_indexed() -> bool {
    FILES_INDEXED.get().copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_static_dir() {
        let dir = default_static_dir();
        assert!(dir.contains("public"));
    }

    #[test]
    fn test_is_port_taken_unlikely() {
        // Port 65432 is unlikely to be in use
        assert!(!is_port_taken(65432));
    }

    #[test]
    fn test_parse_cors_origin_valid() {
        let v = parse_cors_origin("http://localhost:5173").expect("parse ok");
        assert_eq!(v.to_str().unwrap(), "http://localhost:5173");
    }

    #[test]
    fn test_parse_cors_origin_invalid() {
        assert!(parse_cors_origin("not a url").is_err());
        assert!(parse_cors_origin("").is_err());
    }

    #[test]
    fn test_env_u64_with_default() {
        assert_eq!(env_u64("DUAPI_TEST_MISSING_VAR", 30), 30);
        // SAFETY: env mutation in a unit test; serial_test crate is already a dep.
        unsafe { std::env::set_var("DUAPI_TEST_PARSE_OK", "120") };
        assert_eq!(env_u64("DUAPI_TEST_PARSE_OK", 30), 120);
        unsafe { std::env::set_var("DUAPI_TEST_PARSE_BAD", "not-a-number") };
        assert_eq!(env_u64("DUAPI_TEST_PARSE_BAD", 30), 30);
    }
}
