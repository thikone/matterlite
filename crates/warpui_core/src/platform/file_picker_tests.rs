// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/platform/file_picker_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use super::*;

#[test]
fn yaml_file_type_accepts_both_yaml_and_yml() {
    assert_eq!(FileType::Yaml.extensions(), &["yaml", "yml"]);
}

#[test]
fn markdown_file_type_accepts_md_and_markdown() {
    assert_eq!(FileType::Markdown.extensions(), &["md", "markdown"]);
}
