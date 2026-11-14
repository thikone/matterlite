// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};

use crate::de::null_default;
use crate::ids::PostId;
use crate::post::Post;
use crate::user::User;

/// A followed thread with the current user's read state (`server/public/model/thread.go`
/// `ThreadResponse`). `id` is the root post id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadResponse {
    pub id: PostId,
    pub reply_count: i64,
    pub last_reply_at: i64,
    pub last_viewed_at: i64,
    #[serde(deserialize_with = "null_default")]
    pub participants: Vec<User>,
    pub post: Option<Post>,
    pub unread_replies: i64,
    pub unread_mentions: i64,
    pub is_urgent: bool,
    pub delete_at: i64,
}

/// `GET /users/{uid}/teams/{tid}/threads`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadList {
    pub total: i64,
    pub total_unread_threads: i64,
    pub total_unread_mentions: i64,
    pub total_unread_urgent_mentions: i64,
    #[serde(deserialize_with = "null_default")]
    pub threads: Vec<ThreadResponse>,
}
