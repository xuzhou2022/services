//! HTTP surface for the `api` service.
//!
//! The router is built here rather than in `main.rs` so integration tests can
//! exercise routes and middleware without binding a socket.

use axum::{Json, Router, extract::Request, http::StatusCode, routing::get};
use common::ServiceInfo;
use serde::Serialize;
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::{MakeSpan, TraceLayer},
};
use tracing::Span;

pub const INFO: ServiceInfo = ServiceInfo::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Runtime settings, read from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub addr: SocketAddr,
    pub request_timeout: Duration,
}

impl Config {
    /// Reads `HOST` (default `0.0.0.0`), `PORT` (default `3000`), and
    /// `REQUEST_TIMEOUT_SECS` (default `30`).
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::resolve(|key| env::var(key).ok())
    }

    /// Config resolution against an arbitrary lookup, so tests can supply
    /// values without mutating process-wide environment state.
    ///
    /// A variable that is set but unparseable is an error rather than a
    /// fallback to the default: a typo in `PORT` should fail loudly instead of
    /// silently serving somewhere unexpected.
    fn resolve(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let host = match lookup("HOST") {
            Some(raw) => raw
                .parse::<IpAddr>()
                .map_err(|_| ConfigError::Invalid { key: "HOST", raw })?,
            None => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        };
        let port = match lookup("PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| ConfigError::Invalid { key: "PORT", raw })?,
            None => DEFAULT_PORT,
        };
        let timeout_secs = match lookup("REQUEST_TIMEOUT_SECS") {
            Some(raw) => raw.parse::<u64>().map_err(|_| ConfigError::Invalid {
                key: "REQUEST_TIMEOUT_SECS",
                raw,
            })?,
            None => DEFAULT_TIMEOUT_SECS,
        };

        Ok(Self {
            addr: SocketAddr::new(host, port),
            request_timeout: Duration::from_secs(timeout_secs),
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::resolve(|_| None).expect("built-in defaults are valid")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Invalid { key: &'static str, raw: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { key, raw } => write!(f, "{key} is not valid: {raw:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Body of `GET /health`: `{"status":"ok","name":"api","version":"0.1.0"}`.
#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    #[serde(flatten)]
    pub service: ServiceInfo,
}

/// The service's routes, without middleware. Add new endpoints here.
pub fn routes() -> Router {
    Router::new().route("/health", get(health))
}

/// Opens the per-request tracing span with the request ID attached.
///
/// `TraceLayer`'s default span omits it, which would leave the ID visible to
/// the client in the response header but absent from the logs it is meant to
/// correlate.
#[derive(Clone, Copy)]
struct RequestSpan;

impl MakeSpan<axum::body::Body> for RequestSpan {
    fn make_span(&mut self, request: &Request) -> Span {
        // Present because SetRequestIdLayer sits above TraceLayer.
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .and_then(|id| id.header_value().to_str().ok())
            .unwrap_or("unset");

        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id,
        )
    }
}

/// Wraps any router in the shared middleware stack.
///
/// Split from [`router`] so tests can drive the stack against a purpose-built
/// route (a deliberately slow one, for instance) instead of only `/health`.
///
/// Ordering is outside-in, and the placement of propagation is load-bearing:
/// it sits *above* the timeout so that the 408 the timeout synthesizes still
/// carries `x-request-id`. Below the timeout it would only ever see responses
/// the handler actually produced, and every timed-out request would come back
/// untraceable.
pub fn apply_middleware(router: Router, config: &Config) -> Router {
    router.layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::x_request_id())
            .layer(TraceLayer::new_for_http().make_span_with(RequestSpan))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                config.request_timeout,
            )),
    )
}

pub fn router(config: &Config) -> Router {
    apply_middleware(routes(), config)
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: INFO.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_vars(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn info_is_wired_to_package_metadata() {
        assert_eq!(INFO.name, "api");
        assert!(!INFO.version.is_empty());
    }

    #[test]
    fn defaults_apply_when_vars_are_absent() {
        let config = Config::resolve(no_vars).expect("defaults are valid");
        assert_eq!(config.addr, SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)));
        assert_eq!(
            config.request_timeout,
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(config, Config::default());
    }

    #[test]
    fn env_overrides_every_field() {
        let config = Config::resolve(|key| match key {
            "HOST" => Some("127.0.0.1".to_string()),
            "PORT" => Some("8080".to_string()),
            "REQUEST_TIMEOUT_SECS" => Some("5".to_string()),
            _ => None,
        })
        .expect("overrides are valid");
        assert_eq!(config.addr, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(config.request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn unparseable_port_is_rejected() {
        let err = Config::resolve(|key| (key == "PORT").then(|| "http".to_string()))
            .expect_err("bad port must not fall back to the default");
        assert_eq!(
            err,
            ConfigError::Invalid {
                key: "PORT",
                raw: "http".to_string()
            }
        );
        assert_eq!(err.to_string(), r#"PORT is not valid: "http""#);
    }

    #[test]
    fn unparseable_host_is_rejected() {
        let err = Config::resolve(|key| (key == "HOST").then(|| "localhost".to_string()))
            .expect_err("HOST must be an IP address, not a hostname");
        assert_eq!(err.to_string(), r#"HOST is not valid: "localhost""#);
    }

    #[test]
    fn unparseable_timeout_is_rejected() {
        let err = Config::resolve(|key| (key == "REQUEST_TIMEOUT_SECS").then(|| "30s".to_string()))
            .expect_err("timeout is a plain second count");
        assert_eq!(
            err.to_string(),
            r#"REQUEST_TIMEOUT_SECS is not valid: "30s""#
        );
    }
}
