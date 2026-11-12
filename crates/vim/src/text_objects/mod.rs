// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/vim/src/text_objects/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! This module is for text-objects, e.g. `diw` and the like.
//!
//! See https://vimdoc.sourceforge.net/htmldoc/motion.html#text-objects
//! or enter ":help text-objects" in Vim.
mod block;
mod paragraph;
mod quote;
mod word;

pub use paragraph::*;
pub use quote::*;
pub use word::*;

pub use self::block::*;
