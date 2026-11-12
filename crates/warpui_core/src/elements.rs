// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/elements.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! The element library. GUI elements are always compiled; the `tui` feature
//! additively adds the TUI element module alongside them.
mod gui;
pub use gui::*;

pub mod animation;
pub mod shimmer_math;
