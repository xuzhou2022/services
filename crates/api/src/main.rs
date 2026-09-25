use api::{AppState, Config, INFO, LogFormat, router};
use std::{process::ExitCode, time::Duration};
use tokio::{net::TcpListener, signal};
use tracing_subscriber::{EnvFilter, fmt};

const DEFAULT_LOG_FILTER: &str = "info";

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

    let filter = match log_filter() {
        Ok(filter) => filter,
        Err(error) => {
            eprintln!("{} failed to start: {error}", INFO.banner());
            return ExitCode::FAILURE;
        }
    };

    init_tracing(config.log_format, filter);

    match serve(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "{} failed to start", INFO.banner());
            ExitCode::FAILURE
        }
    }
}

/// Builds the log filter from `RUST_LOG`, defaulting to `info` when unset.
///
/// A malformed `RUST_LOG` is an error rather than a silent fallback, matching
/// how every other environment variable is treated. The previous behavior
/// discarded the whole filter on one bad directive, so a typo left the service
/// logging at `info` while appearing to honor the request.
fn log_filter() -> Result<EnvFilter, String> {
    resolve_log_filter(|key| std::env::var(key).ok())
}

/// Split from [`log_filter`] for the same reason `Config::resolve` is: tests
/// can supply a value without mutating process-wide environment state, which
/// would race other tests in the same binary.
fn resolve_log_filter(lookup: impl Fn(&str) -> Option<String>) -> Result<EnvFilter, String> {
    match lookup("RUST_LOG") {
        Some(raw) => EnvFilter::try_new(&raw)
            .map_err(|error| format!("RUST_LOG is not valid: {raw:?} ({error})")),
        None => Ok(EnvFilter::new(DEFAULT_LOG_FILTER)),
    }
}

fn init_tracing(format: LogFormat, filter: EnvFilter) {
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

    let state = AppState::new();
    let drain = config.shutdown_drain;

    axum::serve(listener, router(&config, state.clone()))
        .with_graceful_shutdown(shutdown(state, drain))
        .await?;

    tracing::info!("shutdown complete");
    Ok(())
}

/// Resolves on Ctrl-C or `SIGTERM`, so container stops drain in flight
/// requests instead of cutting them off.
///
/// Readiness flips to false first, then the process keeps serving for
/// `drain`. That window is the point: once this future resolves the server
/// stops accepting connections, so a load balancer polling `/health/ready`
/// would get connection-refused rather than the 503 it needs to see in order
/// to deregister the instance gracefully.
async fn shutdown(state: AppState, drain: Duration) {
    tokio::select! {
        () = ctrl_c() => tracing::info!("received Ctrl-C, draining"),
        () = terminate() => tracing::info!("received SIGTERM, draining"),
    }

    state.set_ready(false);

    if drain.is_zero() {
        tracing::info!("readiness withdrawn, draining immediately");
        return;
    }

    tracing::info!(?drain, "readiness withdrawn, still serving during drain");

    // A second signal cuts the wait short. Otherwise an impatient Ctrl-C is
    // swallowed and the only way out of a long drain is SIGKILL.
    tokio::select! {
        () = tokio::time::sleep(drain) => {}
        () = ctrl_c() => tracing::info!("second signal, ending drain early"),
        () = terminate() => tracing::info!("second signal, ending drain early"),
    }
}

async fn ctrl_c() {
    let _ = signal::ctrl_c().await;
}

#[cfg(unix)]
async fn terminate() {
    match signal::unix::signal(signal::unix::SignalKind::terminate()) {
        Ok(mut stream) => {
            stream.recv().await;
        }
        Err(error) => {
            tracing::warn!(%error, "SIGTERM handler unavailable");
            std::future::pending::<()>().await
        }
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending::<()>().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_rust_log_defaults_to_info() {
        let filter = resolve_log_filter(|_| None).expect("the default is valid");
        assert_eq!(filter.to_string(), DEFAULT_LOG_FILTER);
    }

    #[test]
    fn valid_rust_log_is_honored_not_defaulted() {
        let directives = "warn,api=debug";
        let filter =
            resolve_log_filter(|_| Some(directives.to_string())).expect("directives are valid");

        // Compared against a filter built from the same string, since
        // EnvFilter's Display reorders directives; asserting a literal would
        // be testing its formatting rather than this function.
        assert_eq!(filter.to_string(), EnvFilter::new(directives).to_string());
        assert_ne!(filter.to_string(), DEFAULT_LOG_FILTER);
    }

    #[test]
    fn malformed_rust_log_is_rejected() {
        // The whole filter used to be discarded on one bad directive, so this
        // silently logged at info while looking like it had been honored.
        let error = resolve_log_filter(|_| Some("warn,foo=notalevel".to_string()))
            .expect_err("a bad level must not fall back to the default");
        assert!(
            error.starts_with(r#"RUST_LOG is not valid: "warn,foo=notalevel""#),
            "message should name the offending value, got: {error}"
        );
    }
}
