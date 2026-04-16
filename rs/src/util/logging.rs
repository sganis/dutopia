// rs/src/util/logging.rs

use tracing_subscriber::{fmt, EnvFilter};

pub fn init_tracing(app: &str) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let format = std::env::var("DUTOPIA_LOG_FORMAT").unwrap_or_else(|_| "json".to_string());

    let builder = fmt().with_env_filter(env_filter).with_target(false);

    let result = if format.eq_ignore_ascii_case("json") {
        builder.json().try_init()
    } else {
        builder.try_init()
    };

    if result.is_ok() {
        tracing::info!(app, "logging initialized");
    }
}
