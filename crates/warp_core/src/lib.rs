// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/lib.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

pub mod app_id;
pub mod assertions;
pub mod r#async;
pub mod features;
pub mod paths;
pub mod platform;
pub mod safe_log;
pub use settings;
pub use settings::{
    define_setting, define_settings_group, implement_setting_for_enum, maybe_define_setting,
};
pub mod ui;
pub mod user_preferences;

#[doc(hidden)]
pub use anyhow as __anyhow;
pub use app_id::AppId;
#[doc(hidden)]
pub use warp_errors as __warp_errors;
pub use warpui_core;
