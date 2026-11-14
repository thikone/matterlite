// SPDX-License-Identifier: AGPL-3.0-only

//! What a channel's timeline shows (WebApp.md §9.5): posts with date separators, the "new
//! messages" line, combined join/leave messages and header grouping, as
//! `makeFilterPostsAndAddSeparators`, `makeCombineUserActivityPosts` (`MR/utils/post_list.ts`)
//! and `areConsecutivePostsBySameUser` (`CS/utils/post_utils.ts`) build them.

use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use mm_model::{ChannelId, Post, User};

use crate::store::Store;
use crate::unread::{JOIN_LEAVE_POST_TYPES, is_notification_suppressed};

/// Consecutive posts by one user within this window share a header (`POST_COLLAPSE_TIMEOUT`).
pub const POST_COLLAPSE_MS: i64 = 5 * 60 * 1000;
/// At most this many system posts are combined into one item (`MAX_COMBINED_SYSTEM_POSTS`).
pub const MAX_COMBINED_SYSTEM_POSTS: usize = 100;

/// Membership posts that are combined into one item (`USER_ACTIVITY_POST_TYPES`).
pub const USER_ACTIVITY_POST_TYPES: &[&str] = &[
    "system_add_to_channel",
    "system_join_channel",
    "system_leave_channel",
    "system_remove_from_channel",
    "system_add_to_team",
    "system_join_team",
    "system_leave_team",
    "system_remove_from_team",
];

#[derive(Clone, Debug, PartialEq)]
pub enum TimelineItem {
    /// A day starts (in the user's timezone).
    Date(NaiveDate),
    /// The "New messages" line, above the first post newer than `last_viewed_at`.
    NewMessages { last_viewed_at: i64 },
    Post {
        post: Arc<Post>,
        /// Same author as the post before it within five minutes: no header.
        consecutive: bool,
    },
    /// Consecutive join/leave/add/remove messages as one item ("A, B and 3 others joined").
    UserActivity(Vec<Arc<Post>>),
    /// Missing history between loaded ranges, or older history not loaded yet.
    Hole,
}

#[derive(Clone, Debug, Default)]
pub struct ItemOptions {
    /// The member's `last_viewed_at` captured when the channel was opened (not the updated
    /// one, or the line would vanish at once). `None` shows no line.
    pub last_viewed_at: Option<i64>,
}

impl Store {
    /// The items of a channel, newest first.
    pub fn timeline_items(&self, channel: &ChannelId, options: &ItemOptions) -> Vec<TimelineItem> {
        let Some(timeline) = self.timelines.get(channel) else {
            return Vec::new();
        };
        let zone = self
            .users
            .get(&self.me)
            .map_or(Zone::Local, |me| Zone::of(me));
        let show_join_leave = self.show_join_leave();
        let my_username = self
            .users
            .get(&self.me)
            .map(|me| me.username.clone())
            .unwrap_or_default();

        let mut out: Vec<TimelineItem> = Vec::new();
        let mut last_date: Option<NaiveDate> = None;
        let mut new_messages_added = false;
        // Oldest first, as the webapp builds it.
        for entry in timeline.items().into_iter().rev() {
            let Some(post) = entry.and_then(|id| self.posts.get(id)) else {
                if entry.is_none() {
                    out.push(TimelineItem::Hole);
                    last_date = None;
                }
                continue;
            };
            if post.post_type == "system_ephemeral_add_to_channel" {
                continue;
            }
            if !show_join_leave
                && JOIN_LEAVE_POST_TYPES.contains(&post.post_type.as_str())
                && !is_about(post, &my_username)
            {
                continue;
            }
            let date = zone.date(post.create_at);
            if last_date != Some(date) {
                out.push(TimelineItem::Date(date));
                last_date = Some(date);
            }
            if let Some(last_viewed_at) = options.last_viewed_at
                && last_viewed_at > 0
                && !new_messages_added
                && post.create_at > last_viewed_at
                && (post.user_id != self.me || from_webhook(post))
                && !is_notification_suppressed(post)
            {
                out.push(TimelineItem::NewMessages { last_viewed_at });
                new_messages_added = true;
            }

            if USER_ACTIVITY_POST_TYPES.contains(&post.post_type.as_str()) {
                if let Some(TimelineItem::UserActivity(posts)) = out.last_mut()
                    && posts.len() < MAX_COMBINED_SYSTEM_POSTS
                {
                    posts.push(post.clone());
                } else {
                    out.push(TimelineItem::UserActivity(vec![post.clone()]));
                }
                continue;
            }
            let consecutive = match out.last() {
                Some(TimelineItem::Post { post: previous, .. }) => consecutive(post, previous),
                _ => false,
            };
            out.push(TimelineItem::Post {
                post: post.clone(),
                consecutive,
            });
        }
        out.reverse();
        out
    }

    /// `advanced_settings/join_leave`, default from the server's
    /// `EnableJoinLeaveMessageByDefault` (`shouldShowJoinLeaveMessages`).
    pub fn show_join_leave(&self) -> bool {
        match self.preference("advanced_settings", "join_leave") {
            Some(value) => value == "true",
            None => self.config.get("EnableJoinLeaveMessageByDefault") != Some("false"),
        }
    }
}

/// `areConsecutivePostsBySameUser`.
pub fn consecutive(post: &Post, previous: &Post) -> bool {
    post.user_id == previous.user_id
        && post.create_at - previous.create_at <= POST_COLLAPSE_MS
        && !from_webhook(post)
        && !from_webhook(previous)
        && !post.is_system()
        && !previous.is_system()
        && post.props.get("ai_generated_by") == previous.props.get("ai_generated_by")
}

fn from_webhook(post: &Post) -> bool {
    post.props
        .get("from_webhook")
        .is_some_and(|value| value.as_str().is_some_and(|value| !value.is_empty()))
}

/// A join/leave message about me (never hidden): `username`, `addedUsername` or
/// `removedUsername` in the props.
fn is_about(post: &Post, username: &str) -> bool {
    !username.is_empty()
        && ["username", "addedUsername", "removedUsername"]
            .iter()
            .any(|key| post.props.get(*key).and_then(|value| value.as_str()) == Some(username))
}

/// The timezone dates are shown in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zone {
    Named(Tz),
    /// The user has no zone set: the computer's.
    Local,
}

impl Zone {
    /// `getUserCurrentTimezone`: the automatic zone if `useAutomaticTimezone` is `"true"`,
    /// else the manual one. The webapp sets the automatic zone from the browser; a native
    /// client falls back to the OS zone when it's empty or unknown.
    pub fn of(user: &User) -> Self {
        let name = if user
            .timezone
            .get("useAutomaticTimezone")
            .map(String::as_str)
            == Some("true")
        {
            user.timezone.get("automaticTimezone")
        } else {
            user.timezone.get("manualTimezone")
        };
        name.and_then(|name| name.parse::<Tz>().ok())
            .map_or(Self::Local, Self::Named)
    }

    pub fn date(self, at_ms: i64) -> NaiveDate {
        let utc = DateTime::<Utc>::from_timestamp_millis(at_ms).unwrap_or_default();
        match self {
            Self::Named(tz) => tz.from_utc_datetime(&utc.naive_utc()).date_naive(),
            Self::Local => Local.from_utc_datetime(&utc.naive_utc()).date_naive(),
        }
    }
}

#[cfg(test)]
#[path = "items_tests.rs"]
mod tests;
