use api::{Config, INFO, LogFormat, router};
use std::process::ExitCode;
use tokio::{net::TcpListener, signal};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> ExitCode {
    // Config is read before logging is initialized, since it chooses the log
    // format. A failure here therefore reports on stderr rather than through
    // tracing.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{} failed to start: {error}", INFO.banner());
            return ExitCode::FAILURE;
        }
    };

    init_tracing(config.log_format);

    match serve(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "{} failed to start", INFO.banner());
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let builder = fmt().with_env_filter(filter);

    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().flatten_event(true).init(),
    }
}

async fn serve(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(config.addr).await?;

    // Resolved rather than configured: port 0 binds an OS-assigned port.
    tracing::info!(
        addr = %listener.local_addr()?,
        timeout = ?config.request_timeout,
        "{} listening",
        INFO.banner(),
    );

    axum::serve(listener, router(&config))
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
