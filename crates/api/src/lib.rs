//! HTTP surface for the `api` service.
//!
//! The router is built here rather than in `main.rs` so integration tests can
//! exercise routes without binding a socket.

use axum::{Json, Router, routing::get};
use common::ServiceInfo;
use serde::Serialize;
use std::{
    env,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

pub const INFO: ServiceInfo = ServiceInfo::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

const DEFAULT_PORT: u16 = 3000;

/// Runtime settings, read from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub addr: SocketAddr,
}

impl Config {
    /// Reads `HOST` (default `0.0.0.0`) and `PORT` (default `3000`).
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

        Ok(Self {
            addr: SocketAddr::new(host, port),
        })
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

pub fn router() -> Router {
    Router::new().route("/health", get(health))
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
    }

    #[test]
    fn env_overrides_host_and_port() {
        let config = Config::resolve(|key| match key {
            "HOST" => Some("127.0.0.1".to_string()),
            "PORT" => Some("8080".to_string()),
            _ => None,
        })
        .expect("overrides are valid");
        assert_eq!(config.addr, SocketAddr::from(([127, 0, 0, 1], 8080)));
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
}
