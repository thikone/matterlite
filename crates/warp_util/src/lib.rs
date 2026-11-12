// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_util/src/lib.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Generic path, file and text helpers shared by the framework crates.
pub mod content_version;
pub mod file;
pub mod file_type;
pub mod path;
pub mod standardized_path;
pub mod user_input;

#[cfg(windows)]
pub mod windows;
