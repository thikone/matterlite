// SPDX-License-Identifier: AGPL-3.0-only

//! Viewers for files from chats (MatterLite.md §14), built on `warp_editor`: rendered Markdown
//! and Jupyter notebooks, highlighted code and plain text. This is the M0 read-only spike;
//! editing, find, go to line, the image lightbox and the external-app fallback come in M5.

pub mod file_type;
pub mod model;
pub mod styles;
pub mod view;

pub use file_type::{FileViewKind, file_view_kind};
pub use model::{DocumentFormat, FileViewModel};
pub use view::{FileView, FileViewAction, init};
