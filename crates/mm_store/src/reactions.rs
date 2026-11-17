// SPDX-License-Identifier: AGPL-3.0-only

//! What reacting needs from the store: the emoji I used most recently (the webapp's
//! `recent_emojis` preference, `addRecentEmojis` in `actions/emoji_actions.ts`), my skin tone
//! (`emoji/emoji_skintone`), the server's limit on distinct emoji per post
//! (`UniqueEmojiReactionLimitPerPost`, checked by the webapp's `addReaction`) and the
//! reactions the server confirms.

use mm_model::{PostId, Preference, Reaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Store, StoreDelta};

/// The preference's category; its name is my user id.
pub const RECENT_EMOJIS: &str = "recent_emojis";
/// `MAXIMUM_RECENT_EMOJI`.
pub const MAX_RECENT_EMOJIS: usize = 27;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RecentEmoji {
    name: String,
    #[serde(rename = "usageCount")]
    usage_count: i64,
}

/// The stored list, least used first. Older clients stored plain names
/// (`normalizeRecentEmojisData`).
fn parse_recent(value: &str) -> Vec<RecentEmoji> {
    let Ok(entries) = serde_json::from_str::<Vec<Value>>(value) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            Value::String(name) => Some(RecentEmoji {
                name,
                usage_count: 1,
            }),
            other => serde_json::from_value(other).ok(),
        })
        .collect()
}

impl Store {
    /// The emoji I used, most used first (and of those used as often, the latest first), as
    /// the picker's "Recently Used" shows them.
    pub fn recent_emojis(&self) -> Vec<String> {
        let value = self
            .preference(RECENT_EMOJIS, self.me.as_str())
            .unwrap_or("[]");
        parse_recent(value)
            .into_iter()
            .rev()
            .map(|entry| entry.name)
            .collect()
    }

    /// The `recent_emojis` preference after using `name`: its count goes up and it moves to
    /// the end of its count; at most [`MAX_RECENT_EMOJIS`] are kept.
    pub fn recent_emojis_after(&self, name: &str) -> Preference {
        let value = self
            .preference(RECENT_EMOJIS, self.me.as_str())
            .unwrap_or("[]");
        let mut recent = parse_recent(value);
        let used = match recent.iter().position(|entry| entry.name == name) {
            Some(index) => {
                let entry = recent.remove(index);
                RecentEmoji {
                    usage_count: entry.usage_count + 1,
                    ..entry
                }
            }
            None => RecentEmoji {
                name: name.to_owned(),
                usage_count: 1,
            },
        };
        recent.push(used);
        if recent.len() > MAX_RECENT_EMOJIS {
            recent.drain(..recent.len() - MAX_RECENT_EMOJIS);
        }
        // Stable, as JavaScript's sort.
        recent.sort_by_key(|entry| entry.usage_count);
        Preference {
            user_id: self.me.clone(),
            category: RECENT_EMOJIS.to_owned(),
            name: self.me.to_string(),
            value: serde_json::to_string(&recent).unwrap_or_else(|_| "[]".to_owned()),
        }
    }

    /// My skin tone for emoji with variations (`1F3FB`…`1F3FF`); `None` for the default.
    pub fn skin_tone(&self) -> Option<&str> {
        self.preference("emoji", "emoji_skintone")
            .filter(|tone| !tone.is_empty() && *tone != "default")
    }

    /// Whether `emoji` may be added to the post: it is already there, or the post has fewer
    /// distinct emoji than `UniqueEmojiReactionLimitPerPost` (no limit when the server doesn't
    /// say).
    pub fn reaction_allowed(&self, post: &PostId, emoji: &str) -> bool {
        let Some(limit) = self
            .config
            .int("UniqueEmojiReactionLimitPerPost")
            .filter(|limit| *limit >= 0)
        else {
            return true;
        };
        let Some(metadata) = self.posts.get(post).and_then(|post| post.metadata.as_ref()) else {
            return true;
        };
        let mut names: Vec<&str> = metadata
            .reactions
            .iter()
            .map(|reaction| reaction.emoji_name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        names.contains(&emoji) || (names.len() as i64) < limit
    }

    /// A reaction the server confirmed (added) or removed, before (or instead of) its
    /// `reaction_added`/`reaction_removed`; applying it twice changes nothing.
    pub fn put_reaction(&mut self, reaction: &Reaction, added: bool) -> Vec<StoreDelta> {
        let mut deltas = Vec::new();
        self.reaction(reaction, added, &mut deltas);
        deltas
    }
}

#[cfg(test)]
#[path = "reactions_tests.rs"]
mod tests;
