// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/util/openable_file_type.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Which viewer a file opens in (MatterLite.md §14.2).

use std::path::Path;

pub use warp_util::file_type::{
    is_binary_file, is_file_content_binary, is_jupyter_notebook_file, is_markdown_file,
};

/// Text files above this size open with the system app instead (MatterLite.md §14.2).
pub const MAX_VIEWABLE_TEXT_BYTES: u64 = 20 * 1024 * 1024;

/// How a file is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileViewKind {
    /// Markdown, rendered with a Raw toggle.
    Markdown,
    /// A Jupyter notebook, rendered.
    Notebook,
    /// A file with a known language: highlighted code.
    Code,
    /// Other text (`.txt`, `.csv`, `.log`, …): plain monospace text.
    Text,
    /// An image, for the lightbox.
    Image,
    /// Binary, unknown or too large: open with the system app or save a copy.
    External,
}

impl FileViewKind {
    /// Whether the viewer is a [`crate::FileView`] (everything except images and external
    /// files).
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Self::Markdown | Self::Notebook | Self::Code | Self::Text
        )
    }
}

/// Checks if a file is a code file with language support.
pub fn is_supported_code_file(path: impl AsRef<Path>) -> bool {
    languages::language_by_local_filename(path.as_ref()).is_some()
}

pub fn is_supported_image_file(path: impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg"
            )
        })
        .unwrap_or(false)
}

/// The viewer for `path`, from its name and, for unknown extensions, its first kilobyte. `size`
/// is the file size in bytes.
pub fn file_view_kind(path: &Path, size: u64) -> FileViewKind {
    // Images first: SVG is text, but shows as an image.
    if is_supported_image_file(path) {
        return FileViewKind::Image;
    }
    if size > MAX_VIEWABLE_TEXT_BYTES || is_binary_file(path) {
        return FileViewKind::External;
    }
    if is_jupyter_notebook_file(path) {
        FileViewKind::Notebook
    } else if is_markdown_file(path) {
        FileViewKind::Markdown
    } else if is_supported_code_file(path) {
        FileViewKind::Code
    } else if is_file_content_binary(path) {
        // An unknown extension: trust the content. Unreadable files count as binary.
        FileViewKind::External
    } else {
        FileViewKind::Text
    }
}

#[cfg(test)]
#[path = "file_type_tests.rs"]
mod tests;
