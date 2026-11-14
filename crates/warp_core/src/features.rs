// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/features.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

pub use warp_features::*;

/// Flags MatterLite turns on; the others stay off.
pub const MATTERLITE_FLAGS: &[FeatureFlag] = &[
    FeatureFlag::SettingsFile,
    FeatureFlag::MarkdownTables,
    FeatureFlag::MarkdownMermaid,
];

/// Enables [`MATTERLITE_FLAGS`] and marks the flags initialized. Call once at startup, before
/// anything checks a flag.
pub fn init() {
    for flag in MATTERLITE_FLAGS {
        flag.set_enabled(true);
    }
    mark_initialized();
}
