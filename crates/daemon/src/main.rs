use crab_daemon::server::{DaemonConfig, DaemonServer};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = DaemonConfig::default();

    // Initialize logging — daemon is long-running, use file-based log rotation.
    let _ = std::fs::create_dir_all(&config.log_dir);
    let file_appender = tracing_appender::rolling::daily(&config.log_dir, "daemon.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        port = config.port,
        "crab-daemon starting"
    );

    let server = DaemonServer::new(config);
    tracing::info!(
        status = %server.status().await,
        sessions = server.session_count().await,
        uptime_secs = server.uptime().as_secs(),
        "daemon initialized"
    );

    // Graceful shutdown on Ctrl+C
    let server_handle = &server;
    tokio::select! {
        result = server_handle.run() => {
            if let Err(e) = result {
                tracing::error!(error = %e, "daemon fatal error");
                std::process::exit(1);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!(
                uptime_secs = server_handle.uptime().as_secs(),
                sessions = server_handle.session_count().await,
                "shutting down"
            );
            server_handle.shutdown().await;
        }
    }

    Ok(())
}
