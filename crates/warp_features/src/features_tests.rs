// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warp_features/src/features_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use enum_iterator::all;

use super::*;

#[test]
fn flags_start_disabled() {
    mark_initialized();
    for flag in all::<FeatureFlag>() {
        assert!(
            !flag.is_enabled(),
            "{flag:?} is on before anything enabled it"
        );
    }
}

#[test]
#[should_panic(expected = "override_enabled")]
fn tests_must_not_change_flags_globally() {
    FeatureFlag::MarkdownTables.set_enabled(true);
}
