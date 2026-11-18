// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::null_default;
use crate::ids::{ChannelId, EmojiId, FileId, PostId, UserId};
use crate::user::User;

/// A post (`server/public/model/post.go`). Timestamps are milliseconds since the epoch; `0`
/// means "not set" (`edit_at == 0`: never edited, `delete_at == 0`: not deleted).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Post {
    pub id: PostId,
    pub create_at: i64,
    pub update_at: i64,
    pub edit_at: i64,
    pub delete_at: i64,
    pub is_pinned: bool,
    pub user_id: UserId,
    pub channel_id: ChannelId,
    /// Empty for a root post.
    pub root_id: PostId,
    pub original_id: PostId,
    pub message: String,
    /// Empty for a normal post; `system_*` types are rendered by the client from `props`.
    #[serde(rename = "type")]
    pub post_type: String,
    #[serde(deserialize_with = "null_default")]
    pub props: Map<String, Value>,
    pub hashtags: String,
    #[serde(deserialize_with = "null_default")]
    pub file_ids: Vec<FileId>,
    /// `"{user_id}:{epoch_ms}"` of an optimistic post; the server echoes it back.
    pub pending_post_id: String,
    pub has_reactions: bool,
    pub remote_id: Option<String>,
    pub reply_count: i64,
    pub last_reply_at: i64,
    #[serde(deserialize_with = "null_default")]
    pub participants: Vec<User>,
    pub is_following: Option<bool>,
    pub metadata: Option<PostMetadata>,
}

impl Post {
    pub fn is_root(&self) -> bool {
        self.root_id.is_empty()
    }

    pub fn is_deleted(&self) -> bool {
        self.delete_at != 0
    }

    pub fn is_system(&self) -> bool {
        self.post_type.starts_with("system_")
    }
}

/// Data the server attaches to a post (`server/public/model/post_metadata.go`). Embeds,
/// images, priority and acknowledgements stay untyped until the renderer needs them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostMetadata {
    #[serde(deserialize_with = "null_default")]
    pub embeds: Vec<Value>,
    #[serde(deserialize_with = "null_default")]
    pub emojis: Vec<Emoji>,
    #[serde(deserialize_with = "null_default")]
    pub files: Vec<FileInfo>,
    #[serde(deserialize_with = "null_default")]
    pub images: Map<String, Value>,
    #[serde(deserialize_with = "null_default")]
    pub reactions: Vec<Reaction>,
    pub priority: Option<Value>,
    #[serde(deserialize_with = "null_default")]
    pub acknowledgements: Vec<Value>,
}

/// A page of posts (`server/public/model/post_list.go`). `order` is newest first; `posts` may
/// hold more posts than `order` (root posts of replies).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostList {
    #[serde(deserialize_with = "null_default")]
    pub order: Vec<PostId>,
    #[serde(deserialize_with = "null_default")]
    pub posts: HashMap<PostId, Post>,
    /// Empty when this page reaches the newest post.
    pub next_post_id: PostId,
    /// Empty when this page reaches the oldest post.
    pub prev_post_id: PostId,
    pub has_next: Option<bool>,
    /// Older history is hidden by a license limit when this is set.
    pub first_inaccessible_post_time: i64,
}

impl PostList {
    /// The posts in `order` (newest first), skipping ids missing from `posts`.
    pub fn ordered(&self) -> impl Iterator<Item = &Post> {
        self.order.iter().filter_map(|id| self.posts.get(id))
    }
}

/// A post waiting to be sent at a time (`server/public/model/scheduled_post.go`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduledPost {
    pub id: String,
    pub create_at: i64,
    pub update_at: i64,
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub root_id: PostId,
    pub message: String,
    #[serde(deserialize_with = "null_default")]
    pub file_ids: Vec<FileId>,
    pub scheduled_at: i64,
    pub processed_at: i64,
    /// Why sending failed (`unknown`, `channel_archived`, …); empty while waiting.
    pub error_code: String,
}

/// An emoji reaction (`server/public/model/reaction.go`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub user_id: UserId,
    pub post_id: PostId,
    pub emoji_name: String,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub channel_id: ChannelId,
    pub remote_id: Option<String>,
}

/// An uploaded file (`server/public/model/file_info.go`). `mini_preview` is a base64 tiny JPEG
/// for placeholders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileInfo {
    pub id: FileId,
    pub user_id: UserId,
    pub post_id: PostId,
    pub channel_id: ChannelId,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub name: String,
    pub extension: String,
    pub size: i64,
    pub mime_type: String,
    pub width: i64,
    pub height: i64,
    pub has_preview_image: bool,
    pub mini_preview: Option<String>,
    pub remote_id: Option<String>,
    pub archived: bool,
}

/// A custom emoji (`server/public/model/emoji.go`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Emoji {
    pub id: EmojiId,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub creator_id: UserId,
    pub name: String,
}

#[cfg(test)]
#[path = "post_tests.rs"]
mod tests;
