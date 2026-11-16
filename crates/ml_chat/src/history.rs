// SPDX-License-Identifier: AGPL-3.0-only

//! What the History pane shows, independent of the view (MatterLite.md §10.8): the active
//! chat's loaded messages grouped by day (newest first), which groups are open, matching a
//! query against message text, file names and authors, and the covered range.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::NaiveDate;
use ml_workspace::Matcher;
use mm_model::{ChannelId, Post, PostId};
use mm_store::{JOIN_LEAVE_POST_TYPES, Store};

use crate::timeline::{Clock, date_label};

/// One day of messages, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    pub label: String,
    pub posts: Vec<Arc<Post>>,
}

/// The loaded messages of a chat, newest first: what the timeline shows, without ephemeral
/// posts and, unless the chat shows them, join/leave messages.
pub fn messages(store: &Store, channel: &ChannelId) -> Vec<Arc<Post>> {
    let Some(timeline) = store.timelines.get(channel) else {
        return Vec::new();
    };
    let show_join_leave = store.show_join_leave();
    timeline
        .post_ids()
        .filter_map(|id| store.posts.get(id))
        .filter(|post| post.post_type != "system_ephemeral_add_to_channel")
        .filter(|post| show_join_leave || !JOIN_LEAVE_POST_TYPES.contains(&post.post_type.as_str()))
        .cloned()
        .collect()
}

/// Groups messages (newest first) by day in the user's timezone, newest day first.
pub fn days(posts: &[Arc<Post>], clock: &Clock, today: NaiveDate) -> Vec<Day> {
    let mut days: Vec<Day> = Vec::new();
    for post in posts {
        let date = clock.zone.date(post.create_at);
        match days.last_mut() {
            Some(day) if day.date == date => day.posts.push(post.clone()),
            _ => days.push(Day {
                date,
                label: day_label(date, today),
                posts: vec![post.clone()],
            }),
        }
    }
    days
}

/// "Today", "Yesterday", else the weekday and date ("Mon 9 Nov 2026").
pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    match date_label(date, today).as_str() {
        label @ ("Today" | "Yesterday") => label.to_owned(),
        _ => date.format("%a %-d %b %Y").to_string(),
    }
}

/// Which day groups are open: Today and Yesterday by default, older days closed; what the
/// user changed is kept per chat (as JSON in the UI state).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenDays {
    changed: BTreeMap<NaiveDate, bool>,
}

impl OpenDays {
    pub fn is_open(&self, date: NaiveDate, today: NaiveDate) -> bool {
        self.changed
            .get(&date)
            .copied()
            .unwrap_or(date == today || Some(date) == today.pred_opt())
    }

    pub fn toggle(&mut self, date: NaiveDate, today: NaiveDate) {
        let open = !self.is_open(date, today);
        let default = date == today || Some(date) == today.pred_opt();
        if open == default {
            self.changed.remove(&date);
        } else {
            self.changed.insert(date, open);
        }
    }

    pub fn to_json(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .changed
            .iter()
            .map(|(date, open)| (date.to_string(), (*open).into()))
            .collect();
        serde_json::Value::Object(map).to_string()
    }

    pub fn from_json(json: &str) -> Self {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap_or_default();
        Self {
            changed: map
                .into_iter()
                .filter_map(|(date, open)| Some((date.parse().ok()?, open.as_bool()?)))
                .collect(),
        }
    }
}

/// Whether a post matches: its message, its file names or its author's name.
pub fn post_matches(post: &Post, author: &str, matcher: &Matcher) -> bool {
    matcher.is_match(&post.message)
        || matcher.is_match(author)
        || post
            .metadata
            .iter()
            .flat_map(|metadata| &metadata.files)
            .any(|file| matcher.is_match(&file.name))
}

/// The posts among `posts` that match, by scanning them (regex and case-sensitive queries,
/// and the literal pass that complements the cache's word search).
pub fn scan(store: &Store, posts: &[Arc<Post>], matcher: &Matcher) -> HashSet<PostId> {
    posts
        .iter()
        .filter(|post| {
            let author = store.user_name(&post.user_id).unwrap_or_default();
            post_matches(post, &author, matcher)
        })
        .map(|post| post.id.clone())
        .collect()
}

/// A message as one line: Markdown removed, whitespace folded, cut at `max` characters.
pub fn excerpt(plain_text: &str, max: usize) -> String {
    let folded = plain_text.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= max {
        return folded;
    }
    let mut cut: String = folded.chars().take(max).collect();
    cut.push('…');
    cut
}

/// The footer: what part of the chat's history is on this computer.
pub fn coverage(store: &Store, channel: &ChannelId, clock: &Clock) -> Option<String> {
    let timeline = store.timelines.get(channel)?;
    if timeline.is_known_empty() {
        return Some("No messages yet".to_owned());
    }
    let oldest = timeline
        .post_ids()
        .filter_map(|id| store.posts.get(id))
        .map(|post| post.create_at)
        .min()?;
    let date = clock.zone.date(oldest).format("%-d %b %Y");
    let from_start = timeline.ranges().first().is_some_and(|range| range.start);
    Some(if from_start {
        format!("Local history: all of it, since {date}")
    } else {
        format!("Local history: {date} → now")
    })
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
