// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::path::Path;

use super::*;

const SMALL: u64 = 1024;

#[test]
fn routes_by_extension() {
    assert_eq!(
        file_view_kind(Path::new("README.md"), SMALL),
        FileViewKind::Markdown
    );
    assert_eq!(
        file_view_kind(Path::new("notes.MARKDOWN"), SMALL),
        FileViewKind::Markdown
    );
    assert_eq!(
        file_view_kind(Path::new("analysis.ipynb"), SMALL),
        FileViewKind::Notebook
    );
    assert_eq!(
        file_view_kind(Path::new("main.rs"), SMALL),
        FileViewKind::Code
    );
    assert_eq!(
        file_view_kind(Path::new("config.yaml"), SMALL),
        FileViewKind::Code
    );
    assert_eq!(
        file_view_kind(Path::new("photo.JPG"), SMALL),
        FileViewKind::Image
    );
    assert_eq!(
        file_view_kind(Path::new("logo.svg"), SMALL),
        FileViewKind::Image
    );
    assert_eq!(
        file_view_kind(Path::new("setup.exe"), SMALL),
        FileViewKind::External
    );
    assert_eq!(
        file_view_kind(Path::new("archive.zip"), SMALL),
        FileViewKind::External
    );
}

#[test]
fn large_text_opens_externally() {
    let size = MAX_VIEWABLE_TEXT_BYTES + 1;
    assert_eq!(
        file_view_kind(Path::new("huge.log"), size),
        FileViewKind::External
    );
    assert_eq!(
        file_view_kind(Path::new("huge.md"), size),
        FileViewKind::External
    );
}

#[test]
fn unknown_extensions_are_judged_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("server.log");
    fs::write(&text, "12:00 started\n12:01 ready\n").unwrap();
    let binary = dir.path().join("dump.dat");
    fs::write(&binary, [0u8, 159, 146, 150, 0, 1, 2, 3]).unwrap();

    assert_eq!(file_view_kind(&text, SMALL), FileViewKind::Text);
    assert_eq!(file_view_kind(&binary, SMALL), FileViewKind::External);
    assert_eq!(
        file_view_kind(&dir.path().join("missing.dat"), SMALL),
        FileViewKind::External
    );
}

#[test]
fn text_kinds_use_the_file_view() {
    assert!(FileViewKind::Markdown.is_text());
    assert!(FileViewKind::Code.is_text());
    assert!(!FileViewKind::Image.is_text());
    assert!(!FileViewKind::External.is_text());
}
