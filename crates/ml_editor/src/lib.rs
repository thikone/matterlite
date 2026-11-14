// SPDX-License-Identifier: AGPL-3.0-only

//! The text editor view used for MatterLite's inputs: `EditorView::single_line` for search bars
//! and forms, `EditorView::new` for multi-line input such as the message composer.

#[path = "EditorView.rs"]
mod editor;

pub use editor::*;
/// Carried by [`Event::Navigate`].
pub use warp_editor::editor::NavigationKey;
