// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/settings/src/toml_path_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use super::*;

#[test]
fn storage_key_single_segment() {
    assert_eq!(toml_path_storage_key("font_name"), "font_name");
}

#[test]
fn storage_key_two_segments() {
    assert_eq!(toml_path_storage_key("font.font_name"), "font_name");
}

#[test]
fn storage_key_three_segments() {
    assert_eq!(
        toml_path_storage_key("appearance.text.font_name"),
        "font_name"
    );
}

#[test]
fn hierarchy_single_segment() {
    assert_eq!(toml_path_hierarchy("font_name"), None);
}

#[test]
fn hierarchy_two_segments() {
    assert_eq!(toml_path_hierarchy("font.font_name"), Some("font"));
}

#[test]
fn hierarchy_three_segments() {
    assert_eq!(
        toml_path_hierarchy("appearance.text.font_name"),
        Some("appearance.text")
    );
}

// Verify const evaluation works at compile time.
const _: () = {
    assert!(matches!(toml_path_storage_key("a.b.c").as_bytes(), b"c"));
    assert!(matches!(toml_path_storage_key("key").as_bytes(), b"key"));
};
