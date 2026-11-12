// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_core/src/features.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

pub use warp_features::*;

/// Flags MatterLite turns on. Every other flag inherited from Warp stays off.
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
use warpui_core::platform::menu::{CustomMenuItem, MenuItem, MenuItemPropertyChanges};
fn feature_flag_menu_item(flag: FeatureFlag) -> MenuItem {
    MenuItem::Custom(CustomMenuItem::new(
        &format!("{flag:?}"),
        move |_| {
            // toggling the flag
            flag.set_enabled(!flag.is_enabled())
        },
        move |_props, _ctx| MenuItemPropertyChanges {
            checked: Some(flag.is_enabled()),
            ..Default::default()
        },
        None,
    ))
}

pub fn runtime_flags_menu_items() -> Vec<MenuItem> {
    if !FeatureFlag::RuntimeFeatureFlags.is_enabled() {
        return Vec::new();
    }

    RUNTIME_FEATURE_FLAGS
        .iter()
        .map(|flag| feature_flag_menu_item(*flag))
        .collect()
}
