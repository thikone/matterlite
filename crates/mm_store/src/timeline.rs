// SPDX-License-Identifier: AGPL-3.0-only

//! A channel's (or thread's) loaded posts and which stretches of history they cover.
//!
//! The webapp keeps non-overlapping `PostOrderBlock`s per channel (WebApp.md §9.2). Here the
//! posts live in one ordered set and the blocks are [`Range`]s over it: a range is a stretch
//! known to be contiguous on the server. A post outside every range is stored but not shown,
//! and the space between two ranges is a hole ("Loading missing messages…", MatterLite.md
//! §8.7) that history backfill fills from both ends.

use std::collections::{BTreeSet, HashMap};

use mm_model::PostId;

/// Position in a timeline: `create_at`, then the id to break ties (the server's order).
pub type Key = (i64, PostId);

/// A stretch of history with no posts missing, by its oldest and newest key (inclusive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub oldest: Key,
    pub newest: Key,
    /// Reaches the newest post: live posts are appended here.
    pub recent: bool,
    /// Reaches the start of the channel.
    pub start: bool,
}

impl Range {
    fn contains(&self, key: &Key) -> bool {
        self.oldest <= *key && *key <= self.newest
    }

    fn overlaps(&self, other: &Range) -> bool {
        self.oldest <= other.newest && other.oldest <= self.newest
    }
}

/// A page as the server returned it ([`mm_model::PostList`]).
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// The page's posts, in any order.
    pub keys: Vec<Key>,
    /// `next_post_id == ""`: nothing newer exists.
    pub reaches_newest: bool,
    /// `prev_post_id == ""`: nothing older exists.
    pub reaches_start: bool,
    /// Loaded posts that border the page: the post it was requested relative to
    /// (`before`/`after`) and the server's `prev_post_id`/`next_post_id`. The page joins the
    /// ranges holding them.
    pub adjacent: Vec<Key>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    keys: BTreeSet<Key>,
    create_at: HashMap<PostId, i64>,
    /// Sorted oldest first; never overlapping.
    ranges: Vec<Range>,
    /// A page reported the channel empty.
    empty: bool,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds a timeline from its saved parts (the cache): the posts it holds, its ranges
    /// and whether the channel was seen empty.
    pub fn from_parts(
        keys: impl IntoIterator<Item = Key>,
        mut ranges: Vec<Range>,
        empty: bool,
    ) -> Self {
        let mut timeline = Self {
            empty,
            ..Self::default()
        };
        for key in keys {
            timeline.insert_key(key);
        }
        ranges.sort_by(|a, b| a.oldest.cmp(&b.oldest));
        timeline.ranges = ranges;
        timeline
    }

    /// Whether a page reported the channel empty (saved with the ranges).
    pub fn is_known_empty(&self) -> bool {
        self.empty
    }

    /// Whether the newest posts are loaded, so live posts can be appended.
    pub fn has_recent(&self) -> bool {
        self.empty || self.ranges.last().is_some_and(|range| range.recent)
    }

    /// Whether the whole history is loaded (no holes, both ends reached).
    pub fn is_complete(&self) -> bool {
        self.empty || matches!(self.ranges.as_slice(), [only] if only.recent && only.start)
    }

    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn contains(&self, id: &PostId) -> bool {
        self.create_at.contains_key(id)
    }

    pub fn key_of(&self, id: &PostId) -> Option<Key> {
        self.create_at.get(id).map(|at| (*at, id.clone()))
    }

    /// The oldest loaded post of the range holding the newest posts: the anchor for loading
    /// older posts (`before=`).
    pub fn oldest_of_recent(&self) -> Option<&Key> {
        let range = self.ranges.last().filter(|range| range.recent)?;
        self.keys
            .range(range.oldest.clone()..=range.newest.clone())
            .next()
    }

    /// The newest loaded key overall (for `since` and `after` requests).
    pub fn newest(&self) -> Option<&Key> {
        self.keys.last()
    }

    /// Adds a page and merges it with the ranges it overlaps or borders.
    pub fn insert_page(&mut self, page: Page) {
        for key in &page.keys {
            self.insert_key(key.clone());
        }
        let bounds =
            page.keys
                .iter()
                .chain(&page.adjacent)
                .fold(None::<(Key, Key)>, |bounds, key| match bounds {
                    None => Some((key.clone(), key.clone())),
                    Some((lo, hi)) => Some((lo.min(key.clone()), hi.max(key.clone()))),
                });
        let Some((oldest, newest)) = bounds else {
            // An empty page with no neighbours: the channel has no posts at all.
            if page.reaches_newest && page.reaches_start {
                self.empty = true;
            }
            return;
        };
        self.add_range(Range {
            oldest,
            newest,
            recent: page.reaches_newest,
            start: page.reaches_start,
        });
    }

    /// Adds a post that arrived live (WebSocket or own send). It joins the recent range if the
    /// newest posts are loaded; otherwise it's stored but outside every range, and the next
    /// page load covers it. Returns whether the post is now shown.
    pub fn insert_live(&mut self, key: Key) -> bool {
        if self.empty {
            self.empty = false;
            self.insert_key(key.clone());
            self.ranges.push(Range {
                oldest: key.clone(),
                newest: key,
                recent: true,
                start: true,
            });
            return true;
        }
        self.insert_key(key.clone());
        match self.ranges.last_mut() {
            Some(range) if range.recent => {
                if key > range.newest {
                    range.newest = key;
                } else if key < range.oldest {
                    // Older than everything loaded (a pending post's clock skew): the range
                    // still covers it only if it reaches the start.
                    if range.start {
                        range.oldest = key;
                    } else {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Adds posts from a complete `since` sync (fewer than the server's 1000-post cap):
    /// everything after the recent range's newest post is now known.
    pub fn insert_since(&mut self, keys: impl IntoIterator<Item = Key>) {
        for key in keys {
            self.insert_live(key);
        }
    }

    /// Removes a post (deleted, or a pending post replaced by the server's copy). Range bounds
    /// stay: they are positions, not posts.
    pub fn remove(&mut self, id: &PostId) -> bool {
        match self.create_at.remove(id) {
            Some(at) => self.keys.remove(&(at, id.clone())),
            None => false,
        }
    }

    /// Moves a post whose `create_at` changed (a pending post confirmed with the server's
    /// time).
    pub fn rekey(&mut self, id: &PostId, create_at: i64) {
        if self.remove(id) {
            self.insert_live((create_at, id.clone()));
        }
    }

    /// The posts of every range, newest first, with `None` marking a hole between ranges or
    /// before the oldest range when it doesn't reach the start.
    pub fn items(&self) -> Vec<Option<&PostId>> {
        let mut items = Vec::new();
        for (index, range) in self.ranges.iter().enumerate().rev() {
            if index + 1 < self.ranges.len() || !range.recent {
                // Between ranges, or newer posts not loaded.
                items.push(None);
            }
            items.extend(
                self.keys
                    .range(range.oldest.clone()..=range.newest.clone())
                    .rev()
                    .map(|(_, id)| Some(id)),
            );
        }
        if self.ranges.first().is_some_and(|range| !range.start) {
            items.push(None);
        }
        items
    }

    /// The shown posts, newest first, without hole markers.
    pub fn post_ids(&self) -> impl Iterator<Item = &PostId> {
        self.items().into_iter().flatten()
    }

    /// The holes as (newer side, older side) of each missing stretch, newest first.
    pub fn holes(&self) -> Vec<(Key, Key)> {
        self.ranges
            .windows(2)
            .rev()
            .map(|pair| (pair[1].oldest.clone(), pair[0].newest.clone()))
            .collect()
    }

    fn insert_key(&mut self, key: Key) {
        if let Some(old) = self.create_at.insert(key.1.clone(), key.0)
            && old != key.0
        {
            self.keys.remove(&(old, key.1.clone()));
        }
        self.keys.insert(key);
    }

    fn add_range(&mut self, mut range: Range) {
        // A new range reaching the newest posts takes over that role.
        if range.recent {
            for existing in &mut self.ranges {
                if !existing.overlaps(&range) {
                    existing.recent = false;
                }
            }
        }
        let mut kept = Vec::with_capacity(self.ranges.len() + 1);
        for existing in self.ranges.drain(..) {
            if existing.overlaps(&range) || existing.contains(&range.oldest) {
                range.oldest = range.oldest.min(existing.oldest);
                range.newest = range.newest.max(existing.newest);
                range.recent |= existing.recent;
                range.start |= existing.start;
            } else {
                kept.push(existing);
            }
        }
        kept.push(range);
        kept.sort_by(|a, b| a.oldest.cmp(&b.oldest));
        // Only the newest range can be the recent one, only the oldest the start.
        let last = kept.len() - 1;
        for (index, range) in kept.iter_mut().enumerate() {
            range.recent &= index == last;
            range.start &= index == 0;
        }
        self.ranges = kept;
    }
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
