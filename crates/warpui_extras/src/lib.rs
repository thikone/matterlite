// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warpui_extras/src/lib.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#[cfg(feature = "secure_storage")]
pub mod secure_storage;

#[cfg(feature = "user_preferences")]
pub mod user_preferences;
