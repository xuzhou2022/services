//! Shared building blocks for the services workspace.
//!
//! Anything used by more than one service crate belongs here.

use serde::Serialize;

/// Identifies a running service instance in logs and health responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ServiceInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl ServiceInfo {
    #[must_use]
    pub const fn new(name: &'static str, version: &'static str) -> Self {
        Self { name, version }
    }

    /// Single-line banner emitted on startup.
    ///
    /// ```
    /// use common::ServiceInfo;
    ///
    /// let info = ServiceInfo::new("api", "0.1.0");
    /// assert_eq!(info.banner(), "api v0.1.0");
    /// ```
    #[must_use]
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
