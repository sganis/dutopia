// rs/src/bin/duapi/main.rs
use anyhow::{Context, Result};
use axum::{
    http::Method,
    routing::{get, post},
    Router,
};
use axum_server::tls_rustls::RustlsConfig;
use clap::{ColorChoice, Parser};
use colored::Colorize;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use dutopia::util::print_about;

mod handler;
mod index;
mod item;
mod query;

use handler::{get_files_handler, get_folders_handler, login_handler, users_handler};
use index::InMemoryFSIndex;

static FS_INDEX: OnceLock<InMemoryFSIndex> = OnceLock::new();
static USERS: OnceLock<Vec<String>> = OnceLock::new();

#[derive(Parser, Debug)]
#[command(
    version,
    color = ColorChoice::Auto,
    about = "Disk usage API server with web UI"
)]
struct Args {
    /// Input CSV file path
    input: PathBuf,
    /// UI folder (defaults to STATIC_DIR env var or local public directory)
    #[arg(short, long, value_name = "DIR", env = "STATIC_DIR")]
    static_dir: Option<String>,
    /// Port number (defaults to PORT env var or 8080)
    #[arg(short, long, env = "PORT")]
    port: Option<u16>,
    /// Enable HTTPS with certificate file path
    #[arg(long, value_name = "FILE", env = "TLS_CERT")]
    tls_cert: Option<PathBuf>,
    /// Private key file path (required if tls-cert is set)
    #[arg(long, value_name = "FILE", env = "TLS_KEY")]
    tls_key: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    print_about();

    dotenvy::dotenv().ok();
    if std::env::var("JWT_SECRET").is_err() {
        eprintln!(
            "{}",
            "Warning: JWT_SECRET env var is not set, using default (unsafe)".yellow()
        );
        unsafe {
            std::env::set_var("JWT_SECRET", "1234567890abcdef");
        }
    }

    let args = Args::parse();
    let csv_path = args.input.clone();
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

    let mut idx = InMemoryFSIndex::new();
    let users = idx.load_from_csv(&csv_path)?;

    FS_INDEX.set(idx).expect("FS_INDEX already set");
    USERS.set(users).expect("USERS already set");

    let cors = CorsLayer::new()
        .allow_origin(["http://localhost:5173".parse().unwrap()])
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    let api = Router::new()
        .route("/login", post(login_handler))
        .route("/users", get(users_handler))
        .route("/folders", get(get_folders_handler))
        .route("/files", get(get_files_handler));

    let frontend = ServeDir::new(&static_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", static_dir)));

    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
        .layer(cors);

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    match (args.tls_cert, args.tls_key) {
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
            axum_server::bind_rustls(addr, config)
                .serve(app.into_make_service())
                .await?;
        }
        (None, None) => {
            println!("Serving on http://{addr}  (static dir: {static_dir})");
            axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
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

pub fn get_fs_index() -> &'static InMemoryFSIndex {
    FS_INDEX.get().expect("FS index not initialized")
}

pub fn get_users() -> &'static Vec<String> {
    USERS.get().expect("User list not initialized")
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
}
