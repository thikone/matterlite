// SPDX-License-Identifier: AGPL-3.0-only

//! Unread and mention counts (WebApp.md §10).
//!
//! The server keeps per channel `total_msg_count(_root)` and per member `msg_count(_root)` (the
//! count at the last view) and `mention_count(_root)`. Unread messages are the difference;
//! with Collapsed Reply Threads on, the `_root` counters apply (`calculateUnreadCount` in
//! `MR/utils/channel_utils.ts`).

use mm_model::Post;
use mm_model::{Channel, ChannelMember, UserId};

/// One channel's badge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelUnread {
    pub messages: i64,
    pub mentions: i64,
    pub urgent_mentions: i64,
    /// `mark_unread == "mention"`: only mentions make the channel unread.
    pub muted: bool,
}

impl ChannelUnread {
    pub fn compute(channel: &Channel, member: &ChannelMember, crt: bool) -> Self {
        let (messages, mentions) = if crt {
            (
                channel.total_msg_count_root - member.msg_count_root,
                member.mention_count_root,
            )
        } else {
            (
                channel.total_msg_count - member.msg_count,
                member.mention_count,
            )
        };
        Self {
            messages: messages.max(0),
            mentions: mentions.max(0),
            urgent_mentions: member.urgent_mention_count.max(0),
            muted: member.is_muted(),
        }
    }

    /// Whether the channel shows as unread (bold) in the sidebar.
    pub fn is_unread(&self) -> bool {
        self.mentions > 0 || (!self.muted && self.messages > 0)
    }
}

/// A team's badge (`getTeamsUnreadStatuses`): channel counts summed over the team's
/// non-archived channels (DMs and GMs belong to no team), plus followed threads under CRT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TeamUnread {
    /// Unread messages in channels that aren't muted.
    pub messages: i64,
    /// Mentions in all channels, muted ones included.
    pub mentions: i64,
    pub urgent_mentions: i64,
    /// Threads with unread replies (CRT only).
    pub threads: i64,
    pub thread_mentions: i64,
    pub thread_urgent_mentions: i64,
}

impl TeamUnread {
    pub fn add_channel(&mut self, unread: ChannelUnread) {
        if !unread.muted {
            self.messages += unread.messages;
        }
        self.mentions += unread.mentions;
        self.urgent_mentions += unread.urgent_mentions;
    }

    pub fn is_unread(&self) -> bool {
        self.messages > 0 || self.threads > 0 || self.mentions > 0 || self.thread_mentions > 0
    }

    pub fn total_mentions(&self) -> i64 {
        self.mentions + self.thread_mentions
    }
}

/// System post types the server leaves out of the message counts (`JoinLeaveMessagePostTypes`
/// in `server/public/model/post.go`).
pub const JOIN_LEAVE_POST_TYPES: &[&str] = &[
    "system_join_leave",
    "system_add_remove",
    "system_join_channel",
    "system_leave_channel",
    "system_join_team",
    "system_leave_team",
    "system_add_to_channel",
    "system_remove_from_channel",
    "system_add_to_team",
    "system_remove_from_team",
];

/// Whether the server counted the post in `total_msg_count` (`ExcludesFromChannelMessageCount`):
/// join/leave messages and posts with `silent_notification` (unless `force_notification`) are
/// not counted.
pub fn counts_as_message(post: &Post) -> bool {
    !JOIN_LEAVE_POST_TYPES.contains(&post.post_type.as_str()) && !is_notification_suppressed(post)
}

/// `silent_notification` without `force_notification` (`IsNotificationSuppressed`).
pub fn is_notification_suppressed(post: &Post) -> bool {
    let forced = match post.props.get("force_notification") {
        Some(serde_json::Value::Bool(forced)) => *forced,
        Some(serde_json::Value::String(text)) => !text.is_empty(),
        _ => false,
    };
    !forced && post.props.get("silent_notification") == Some(&serde_json::Value::Bool(true))
}

/// Whether a new post marks the channel read for its author: own posts that aren't system
/// messages or webhook posts (`setChannelReadAndViewed` in `CS/actions/new_post.ts`).
pub fn is_own_message(post: &Post, me: &UserId) -> bool {
    post.user_id == *me && !post.is_system() && !is_from_webhook(post)
}

fn is_from_webhook(post: &Post) -> bool {
    matches!(post.props.get("from_webhook"), Some(serde_json::Value::String(value)) if value == "true")
}

/// `metadata.priority.priority == "urgent"`.
pub fn is_urgent(post: &Post) -> bool {
    post.metadata
        .as_ref()
        .and_then(|metadata| metadata.priority.as_ref())
        .and_then(|priority| priority.get("priority"))
        .and_then(serde_json::Value::as_str)
        == Some("urgent")
}

#[cfg(test)]
#[path = "unread_tests.rs"]
mod tests;
