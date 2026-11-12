// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/test.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use ctor::ctor;

// Initialize the logger before running tests.
#[ctor]
fn init() {
    simplelog::SimpleLogger::init(simplelog::LevelFilter::Info, simplelog::Config::default())
        .unwrap()
}
