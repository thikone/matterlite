// SPDX-License-Identifier: AGPL-3.0-only

//! The server version string of the `X-Version-Id` header and the WebSocket `hello` event.
//!
//! The server writes `<version>.<build number>.<client config hash>.<licensed>`
//! (`server/channels/web/handlers.go`), e.g. `12.2.0.123.a16e….false`, and a dev build has an
//! empty build number (`12.2.0..a16e….false`). A change of the config hash means the client
//! config changed and should be fetched again.

use std::fmt;

/// A parsed server version string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerVersion {
    /// `major.minor.patch`, e.g. `12.2.0`.
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    /// The build number; empty for builds from source.
    pub build: String,
    /// The hash of the server's client config.
    pub config_hash: String,
    pub licensed: bool,
}

impl ServerVersion {
    /// Parses the header value; `None` if it doesn't start with `major.minor.patch`.
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.trim().splitn(4, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        // The build number may itself contain dots, so the hash and flag come from the right.
        let rest = parts.next().unwrap_or("");
        let mut tail = rest.rsplitn(3, '.');
        let (licensed, config_hash, build) = match (tail.next(), tail.next(), tail.next()) {
            (Some(licensed), Some(hash), build) => (licensed, hash, build.unwrap_or("")),
            _ => ("false", "", rest),
        };
        Some(Self {
            major,
            minor,
            patch,
            build: build.to_owned(),
            config_hash: config_hash.to_owned(),
            licensed: licensed == "true",
        })
    }

    /// Whether this server is at least `major.minor.patch`.
    pub fn at_least(&self, major: u32, minor: u32, patch: u32) -> bool {
        (self.major, self.minor, self.patch) >= (major, minor, patch)
    }
}

impl fmt::Display for ServerVersion {
    /// `12.2.0`, or `12.2.0 (build 123)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.build.is_empty() {
            write!(f, " (build {})", self.build)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;
