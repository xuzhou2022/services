//! Shared building blocks for the services workspace.
//!
//! Anything used by more than one service crate belongs here.

use serde::Serialize;

/// Identifies a running service instance in logs and health responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl ServiceInfo {
    pub const fn new(name: &'static str, version: &'static str) -> Self {
        Self { name, version }
    }

    /// Single-line banner emitted on startup, e.g. `api v0.1.0`.
    pub fn banner(&self) -> String {
        format!("{} v{}", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_formats_name_and_version() {
        let info = ServiceInfo::new("api", "0.1.0");
        assert_eq!(info.banner(), "api v0.1.0");
    }
}
