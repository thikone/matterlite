// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/assets/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use std::borrow::Cow;

use anyhow::{Result, anyhow};
pub mod asset_cache;

impl AssetProvider for () {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        Err(anyhow!(
            "get called on empty asset provider with \"{}\"",
            path
        ))
    }
}

pub trait AssetProvider: 'static {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>>;
}
