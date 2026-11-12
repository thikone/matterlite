// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/platform.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use line_ending::LineEnding;

#[derive(Debug, Clone)]
pub enum SessionPlatform {
    MSYS2,
    WSL,
    Native,
    /// A shell running inside a Linux Docker sandbox container.
    DockerSandbox,
}

impl SessionPlatform {
    #[allow(clippy::disallowed_methods)]
    pub fn default_line_ending(&self) -> LineEnding {
        match self {
            SessionPlatform::MSYS2 | SessionPlatform::WSL | SessionPlatform::DockerSandbox => {
                LineEnding::LF
            }
            SessionPlatform::Native => LineEnding::from_current_platform(),
        }
    }
}
