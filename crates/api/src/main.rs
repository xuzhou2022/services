use api::{Config, INFO, router};
use std::process::ExitCode;
use tokio::{net::TcpListener, signal};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> ExitCode {
    fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    match serve().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "{} failed to start", INFO.banner());
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    let listener = TcpListener::bind(config.addr).await?;

    // Resolved rather than configured: port 0 binds an OS-assigned port.
    tracing::info!(addr = %listener.local_addr()?, "{} listening", INFO.banner());

    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown())
        .await?;

    tracing::info!("shutdown complete");
    Ok(())
}

/// Resolves on Ctrl-C or `SIGTERM`, so container stops drain in flight
/// requests instead of cutting them off.
async fn shutdown() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => tracing::warn!(%error, "SIGTERM handler unavailable"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl-C, draining"),
        () = terminate => tracing::info!("received SIGTERM, draining"),
    }
}
