// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui/src/windowing/winit/linux/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

mod app;
pub mod clipboard;
mod cursor_theme;
mod window_manager;
mod zbus;

pub use app::{maybe_register_xlib_error_hook, take_encountered_bad_match_from_dri3_fence_from_fd};
pub use clipboard::*;
pub use cursor_theme::*;
pub(crate) use window_manager::*;
pub use zbus::*;
