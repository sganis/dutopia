// rs/src/bin/duapi/shutdown.rs

/// Future that completes when the process receives Ctrl+C (SIGINT) or SIGTERM (Unix).
pub async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        if let Err(e) = signal::ctrl_c().await {
            tracing::warn!(error = %e, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl+C, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::shutdown_signal;
    use std::time::Duration;

    /// Smoke test: the future does not complete on its own within a short window.
    #[tokio::test]
    async fn test_shutdown_signal_pending_without_signal() {
        let res = tokio::time::timeout(Duration::from_millis(50), shutdown_signal()).await;
        assert!(res.is_err(), "shutdown_signal completed without a signal");
    }
}
