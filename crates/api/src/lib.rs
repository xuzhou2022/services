//! HTTP surface for the `api` service.
//!
//! The router is built here rather than in `main.rs` so integration tests can
//! exercise routes and middleware without binding a socket.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    routing::get,
};
use common::ServiceInfo;
use serde::Serialize;
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnResponse, MakeSpan, TraceLayer},
};
use tracing::{Level, Span};

pub const INFO: ServiceInfo = ServiceInfo::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

const DEFAULT_PORT: u16 = 3000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_DRAIN_SECS: u64 = 5;

/// How startup and request logs are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable, for local development.
    #[default]
    Text,
    /// One JSON object per line, for log aggregators.
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            _ => Err(()),
        }
    }
}

/// Runtime settings, read from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub addr: SocketAddr,
    pub request_timeout: Duration,
    pub log_format: LogFormat,
    /// How long to keep serving after readiness is withdrawn, giving a load
    /// balancer time to notice the 503 and stop sending new work.
    pub shutdown_drain: Duration,
}

impl Config {
    /// Reads `HOST` (default `0.0.0.0`), `PORT` (default `3000`),
    /// `REQUEST_TIMEOUT_SECS` (default `30`), `LOG_FORMAT` (`text` or `json`,
    /// default `text`), and `SHUTDOWN_DRAIN_SECS` (default `5`).
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

        let log_format = match lookup("LOG_FORMAT") {
            Some(raw) => raw.parse().map_err(|()| ConfigError::Invalid {
                key: "LOG_FORMAT",
                raw,
            })?,
            None => LogFormat::default(),
        };

        let drain_secs = match lookup("SHUTDOWN_DRAIN_SECS") {
            Some(raw) => raw.parse::<u64>().map_err(|_| ConfigError::Invalid {
                key: "SHUTDOWN_DRAIN_SECS",
                raw,
            })?,
            None => DEFAULT_DRAIN_SECS,
        };

        Ok(Self {
            addr: SocketAddr::new(host, port),
            request_timeout: Duration::from_secs(timeout_secs),
            log_format,
            shutdown_drain: Duration::from_secs(drain_secs),
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

/// Body shared by all three health endpoints, e.g.
/// `{"status":"ok","name":"api","version":"0.1.0"}`.
///
/// `status` is `ok` except on `/health/ready` during shutdown, where it is
/// `shutting_down` alongside a 503.
#[derive(Debug, Serialize)]
pub struct Health {
    pub status: &'static str,
    #[serde(flatten)]
    pub service: ServiceInfo,
}

/// Tracks whether this instance should be receiving traffic.
///
/// Liveness and readiness answer different questions. Liveness is "is the
/// process working" — a failure means restart me. Readiness is "should I get
/// new requests" — during shutdown the answer is no, but the process is
/// perfectly healthy and still finishing in-flight work. Conflating them
/// means an orchestrator restarts a draining instance instead of routing
/// around it.
#[derive(Debug, Clone)]
pub struct AppState {
    ready: Arc<AtomicBool>,
}

/// Deliberately not derived: `AtomicBool::default()` is `false`, which would
/// make a default-constructed service permanently unready.
impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(true)),
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Called when shutdown begins, before in-flight requests are drained.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }
}

/// The service's routes, without middleware. Add new endpoints here.
pub fn routes(state: AppState) -> Router {
    Router::new()
        // `/health` predates the split and behaves as liveness, so it is an
        // alias rather than a third implementation that could drift.
        .route("/health", get(live))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(state)
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
/// Ordering is outside-in, and the placement of ID propagation and the
/// `nosniff` header is load-bearing: both sit *above* the timeout and the
/// panic catcher, so the 408 and 500 those synthesize carry them too. Below,
/// they would only ever see responses a handler actually produced, leaving
/// every timed-out or panicking request untraceable.
pub fn apply_middleware(router: Router, config: &Config) -> Router {
    router.layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::x_request_id())
            // Above the timeout and panic layers so the synthesized 408 and
            // 500 carry it too, not just handler responses.
            .layer(SetResponseHeaderLayer::overriding(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ))
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(RequestSpan)
                    // tower-http emits this at DEBUG, so with the default
                    // `info` filter the service logged nothing per request.
                    // One INFO line per response is the access log.
                    .on_response(DefaultOnResponse::new().level(Level::INFO)),
            )
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                config.request_timeout,
            ))
            // Innermost, so the 500 it produces still travels back out through
            // the trace and propagation layers. Without it a panicking handler
            // drops the connection: no status, no access-log line, nothing for
            // the client or the logs to go on.
            .layer(CatchPanicLayer::new()),
    )
}

pub fn router(config: &Config, state: AppState) -> Router {
    apply_middleware(routes(state), config)
}

/// Liveness: the process is running and serving. Failing this means restart.
/// Also serves `/health`.
async fn live() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: INFO,
    })
}

/// Readiness: whether to send this instance new traffic. Returns 503 once
/// shutdown has begun so a load balancer drains it while it finishes what it
/// already accepted.
async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let (code, status) = if state.is_ready() {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
    };

    (
        code,
        Json(Health {
            status,
            service: INFO,
        }),
    )
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
        assert_eq!(config.log_format, LogFormat::Text);
        assert_eq!(
            config.shutdown_drain,
            Duration::from_secs(DEFAULT_DRAIN_SECS)
        );
        assert_eq!(config, Config::default());
    }

    #[test]
    fn env_overrides_every_field() {
        let config = Config::resolve(|key| match key {
            "HOST" => Some("127.0.0.1".to_string()),
            "PORT" => Some("8080".to_string()),
            "REQUEST_TIMEOUT_SECS" => Some("5".to_string()),
            "LOG_FORMAT" => Some("json".to_string()),
            "SHUTDOWN_DRAIN_SECS" => Some("0".to_string()),
            _ => None,
        })
        .expect("overrides are valid");
        assert_eq!(config.addr, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(config.request_timeout, Duration::from_secs(5));
        assert_eq!(config.log_format, LogFormat::Json);
        // 0 is the documented escape hatch for an instant local Ctrl-C, and
        // has to survive as 0 rather than falling back to the default.
        assert_eq!(config.shutdown_drain, Duration::ZERO);
    }

    #[test]
    fn unknown_log_format_is_rejected() {
        let err = Config::resolve(|key| (key == "LOG_FORMAT").then(|| "logfmt".to_string()))
            .expect_err("only text and json are supported");
        assert_eq!(err.to_string(), r#"LOG_FORMAT is not valid: "logfmt""#);
    }

    #[test]
    fn unparseable_drain_is_rejected() {
        let err = Config::resolve(|key| (key == "SHUTDOWN_DRAIN_SECS").then(|| "5s".to_string()))
            .expect_err("drain is a plain second count");
        assert_eq!(err.to_string(), r#"SHUTDOWN_DRAIN_SECS is not valid: "5s""#);
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
