// SPDX-License-Identifier: AGPL-3.0-only

//! Regenerates the emoji index and images (MatterLite.md §12, WebApp.md §11.3):
//!
//! ```text
//! cargo run -p mm_emoji --example generate --release -- \
//!     <mattermost>/webapp/channels/src/utils/emoji.json \
//!     <emoji-datasource-google 6.1.1>/img/google/64
//! ```
//!
//! The index comes from the webapp's `emoji.json` (generated from `emoji-datasource` 6.1.1 with
//! Mattermost's extra short names and one entry per skin tone), so names match what the
//! webapp and the server use. The images are Google's Noto emoji from
//! `emoji-datasource-google` 6.1.1 (Apache-2.0), scaled to 32 px. The webapp's own images are
//! Apple's, which may not be redistributed.
//!
//! Writes `crates/mm_emoji/data/emoji.json` and `assets/emoji/<unified>.png`; run from the
//! repository root.

use std::collections::BTreeMap;
use std::path::Path;

use image::ImageEncoder as _;
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use serde::{Deserialize, Serialize};

/// Image size in pixels: sharp for inline emoji on 1x and 2x screens, and the size of
/// emoji-only messages on 1x screens.
const SIZE: u32 = 32;

#[derive(Deserialize)]
struct WebappEmoji {
    unified: String,
    short_names: Vec<String>,
    category: String,
    #[serde(default)]
    skins: Option<Vec<String>>,
}

/// One entry of `data/emoji.json`; see `mm_emoji::Emoji`.
#[derive(Serialize)]
struct Entry {
    /// Short names, the first is the canonical one.
    n: Vec<String>,
    /// Unified code points, e.g. `1F44B-1F3FB`.
    u: String,
    /// Category.
    c: String,
    /// The skin tone code point of a skin variant.
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [emoji_json, images] = args.as_slice() else {
        eprintln!("usage: generate <webapp emoji.json> <emoji-datasource-google img/google/64>");
        std::process::exit(2);
    };
    let webapp: Vec<WebappEmoji> =
        serde_json::from_str(&std::fs::read_to_string(emoji_json).expect("read emoji.json"))
            .expect("parse emoji.json");
    let out_dir = Path::new("assets/emoji");
    std::fs::create_dir_all(out_dir).expect("create assets/emoji");
    let mut entries = Vec::new();
    let mut categories: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped = Vec::new();
    for emoji in webapp {
        if emoji.unified.is_empty() {
            // `mattermost`, a custom image only the webapp has.
            skipped.push(emoji.short_names[0].clone());
            continue;
        }
        let file = format!("{}.png", emoji.unified.to_lowercase());
        let source = Path::new(images).join(&file);
        let Ok(image) = image::open(&source) else {
            skipped.push(emoji.short_names[0].clone());
            continue;
        };
        let resized = image
            .resize_exact(SIZE, SIZE, FilterType::Lanczos3)
            .to_rgba8();
        let mut bytes = Vec::new();
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Best, PngFilter::Adaptive)
            .write_image(&resized, SIZE, SIZE, image::ExtendedColorType::Rgba8)
            .expect("encode image");
        std::fs::write(out_dir.join(&file), bytes).expect("write image");
        *categories.entry(emoji.category.clone()).or_default() += 1;
        entries.push(Entry {
            n: emoji.short_names,
            u: emoji.unified,
            c: emoji.category,
            s: emoji.skins.and_then(|skins| skins.into_iter().next()),
        });
    }
    let json = serde_json::to_string(&entries).expect("serialize");
    std::fs::write("crates/mm_emoji/data/emoji.json", json + "\n").expect("write index");
    println!("{} emoji, skipped {skipped:?}", entries.len());
    for (category, count) in categories {
        println!("  {category}: {count}");
    }
}
