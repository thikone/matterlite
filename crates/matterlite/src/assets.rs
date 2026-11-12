// SPDX-License-Identifier: AGPL-3.0-only

use std::borrow::Cow;

use anyhow::{Result, anyhow};
use rust_embed::RustEmbed;
use warpui::AssetProvider;

/// Files under the repository's `assets/` directory, embedded into the binary.
#[derive(Clone, Copy, RustEmbed)]
#[folder = "../../assets"]
pub struct Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> Result<Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|file| file.data)
            .ok_or_else(|| anyhow!("no asset exists at path {path}"))
    }
}
