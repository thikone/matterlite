// SPDX-License-Identifier: AGPL-3.0-only

//! The emoji the webapp knows (MatterLite.md §12, WebApp.md §11.3): short names, Unicode
//! sequences, categories and skin-tone variants, from the webapp's `emoji.json`
//! (`emoji-datasource` 6.1.1 plus Mattermost's extra short names), and the image of each in
//! the application assets (`emoji/<unified>.png`, Google's Noto emoji at 32 px). Also the text
//! emoticons the webapp turns into emoji (`:)`, `<3`, …).
//!
//! Regenerate the data and images with `examples/generate.rs`. No UI dependency.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// One system emoji.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emoji {
    /// Short names; the first is the canonical one (`+1`, `thumbsup`).
    pub short_names: Vec<String>,
    /// Unified code points, e.g. `1F44B-1F3FB`.
    pub unified: String,
    /// The emoji as text.
    pub text: String,
    /// e.g. `smileys-emotion`, `people-body`.
    pub category: String,
    /// For a skin-tone variant, the tone's code point (`1F3FB`…`1F3FF`).
    pub skin_tone: Option<String>,
}

impl Emoji {
    pub fn name(&self) -> &str {
        &self.short_names[0]
    }

    /// The image's asset path.
    pub fn image(&self) -> String {
        format!("emoji/{}.png", self.unified.to_lowercase())
    }
}

#[derive(Deserialize)]
struct Entry {
    n: Vec<String>,
    u: String,
    c: String,
    #[serde(default)]
    s: Option<String>,
}

struct Index {
    emoji: Vec<Emoji>,
    by_name: HashMap<String, usize>,
    /// Keyed by the text without variation selectors (U+FE0F).
    by_text: HashMap<String, usize>,
    longest: usize,
}

const VARIATION_SELECTOR: char = '\u{FE0F}';
const KEYCAP: char = '\u{20E3}';

static INDEX: LazyLock<Index> = LazyLock::new(|| {
    let entries: Vec<Entry> =
        serde_json::from_str(include_str!("../data/emoji.json")).expect("emoji index parses");
    let emoji: Vec<Emoji> = entries
        .into_iter()
        .map(|entry| Emoji {
            text: unified_to_text(&entry.u),
            short_names: entry.n,
            unified: entry.u,
            category: entry.c,
            skin_tone: entry.s,
        })
        .collect();
    let mut by_name = HashMap::new();
    let mut by_text = HashMap::new();
    let mut longest = 0;
    for (index, emoji) in emoji.iter().enumerate() {
        for name in &emoji.short_names {
            by_name.entry(name.clone()).or_insert(index);
        }
        let key: String = emoji
            .text
            .chars()
            .filter(|c| *c != VARIATION_SELECTOR)
            .collect();
        longest = longest.max(key.chars().count());
        by_text.entry(key).or_insert(index);
    }
    Index {
        emoji,
        by_name,
        by_text,
        longest,
    }
});

fn unified_to_text(unified: &str) -> String {
    unified
        .split('-')
        .filter_map(|code| u32::from_str_radix(code, 16).ok())
        .filter_map(char::from_u32)
        .collect()
}

/// Every emoji, in the webapp's order.
pub fn all() -> &'static [Emoji] {
    &INDEX.emoji
}

/// The emoji with this short name (`smile`, `+1`, `wave_light_skin_tone`).
pub fn by_name(name: &str) -> Option<&'static Emoji> {
    INDEX.by_name.get(name).map(|index| &INDEX.emoji[*index])
}

/// The emoji at the start of `text`, and its length in bytes: the longest known sequence.
/// Characters that are text by default (©, ❤, digits) count only with the emoji variation
/// selector or as keycaps, as they appear when typed as emoji.
pub fn at_start(text: &str) -> Option<(&'static Emoji, usize)> {
    let mut chars = text.chars();
    let first = chars.next()?;
    // Plain ASCII is never an emoji on its own; keycaps continue with U+FE0F or U+20E3.
    if first.is_ascii()
        && !chars
            .next()
            .is_some_and(|c| c == VARIATION_SELECTOR || c == KEYCAP)
    {
        return None;
    }
    let mut ends: Vec<usize> = text
        .char_indices()
        .map(|(index, c)| index + c.len_utf8())
        .take(INDEX.longest + 4)
        .collect();
    while let Some(end) = ends.pop() {
        let candidate = &text[..end];
        let key: String = candidate
            .chars()
            .filter(|c| *c != VARIATION_SELECTOR)
            .collect();
        if key.is_empty() {
            continue;
        }
        let Some(index) = INDEX.by_text.get(&key) else {
            continue;
        };
        let emoji = &INDEX.emoji[*index];
        let text_by_default = key.chars().count() == 1
            && emoji.unified.contains("FE0F")
            && !candidate.contains(VARIATION_SELECTOR);
        if text_by_default {
            continue;
        }
        // A variation selector right after belongs to the emoji.
        let end = if text[end..].starts_with(VARIATION_SELECTOR) {
            end + VARIATION_SELECTOR.len_utf8()
        } else {
            end
        };
        return Some((emoji, end));
    }
    None
}

/// The text emoticons the webapp shows as emoji (`emoticonPatterns` in `utils/emoticons.tsx`):
/// the emoticon (lower case; matched ignoring case) and the emoji's short name.
pub const EMOTICONS: &[(&str, &str)] = &[
    (":)", "slightly_smiling_face"),
    (":-)", "slightly_smiling_face"),
    (";)", "wink"),
    (";-)", "wink"),
    (":o", "open_mouth"),
    (":-o", "scream"),
    (":]", "smirk"),
    (":-]", "smirk"),
    (":d", "smile"),
    (":-d", "smile"),
    ("x-d", "stuck_out_tongue_closed_eyes"),
    (":p", "stuck_out_tongue"),
    (":-p", "stuck_out_tongue"),
    (":@", "rage"),
    (":-@", "rage"),
    (":[", "rage"),
    (":-[", "rage"),
    (":(", "slightly_frowning_face"),
    (":-(", "slightly_frowning_face"),
    (":'(", "cry"),
    (":'-(", "cry"),
    (":`(", "cry"),
    (":/", "confused"),
    (":-/", "confused"),
    (":s", "confounded"),
    (":-s", "confounded"),
    (":|", "neutral_face"),
    (":-|", "neutral_face"),
    (":$", "flushed"),
    (":-$", "flushed"),
    (":-x", "mask"),
    ("<3", "heart"),
    ("</3", "broken_heart"),
];

/// The emoticon at the start of `text` (the longest), with its emoji name and length in bytes.
/// After it must come the end, a space or punctuation, never a letter or digit (`:pizza`).
/// The caller checks that no word character comes before it.
pub fn emoticon_at_start(text: &str) -> Option<(&'static str, usize)> {
    let (emoticon, name) = EMOTICONS
        .iter()
        .filter(|(emoticon, _)| {
            text.get(..emoticon.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(emoticon))
        })
        .max_by_key(|(emoticon, _)| emoticon.len())?;
    let rest = &text[emoticon.len()..];
    if rest.chars().next().is_some_and(char::is_alphanumeric) {
        return None;
    }
    Some((name, emoticon.len()))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
