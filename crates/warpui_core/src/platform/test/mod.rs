// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/platform/test/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

mod app;
mod delegate;

pub use app::App;
pub(crate) use delegate::WindowManager;
pub use delegate::{AppDelegate, FontDB, IntegrationTestDelegate};
