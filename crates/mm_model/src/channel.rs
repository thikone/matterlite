// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{null_default, string_map};
use crate::ids::{CategoryId, ChannelId, TeamId, UserId};

/// The channel kind (`type` on the wire). Unknown kinds are kept as-is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ChannelType {
    #[default]
    Open,
    Private,
    Direct,
    Group,
    Other(String),
}

impl ChannelType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "O",
            Self::Private => "P",
            Self::Direct => "D",
            Self::Group => "G",
            Self::Other(other) => other,
        }
    }

    /// Direct and group messages belong to no team (`team_id == ""`).
    pub fn is_direct_or_group(&self) -> bool {
        matches!(self, Self::Direct | Self::Group)
    }
}

impl From<String> for ChannelType {
    fn from(value: String) -> Self {
        match value.as_str() {
            "O" => Self::Open,
            "P" => Self::Private,
            "D" => Self::Direct,
            "G" => Self::Group,
            _ => Self::Other(value),
        }
    }
}

impl From<ChannelType> for String {
    fn from(value: ChannelType) -> Self {
        match value {
            ChannelType::Other(other) => other,
            known => known.as_str().to_owned(),
        }
    }
}

/// A channel (`server/public/model/channel.go`). A DM's `name` is `<user id>__<user id>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: ChannelId,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub team_id: TeamId,
    #[serde(rename = "type")]
    pub channel_type: ChannelType,
    pub display_name: String,
    pub name: String,
    pub header: String,
    pub purpose: String,
    pub last_post_at: i64,
    pub total_msg_count: i64,
    pub total_msg_count_root: i64,
    pub last_root_post_at: i64,
    pub extra_update_at: i64,
    pub creator_id: UserId,
    pub scheme_id: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub props: Map<String, Value>,
    pub group_constrained: Option<bool>,
    pub shared: Option<bool>,
}

impl Channel {
    pub fn is_deleted(&self) -> bool {
        self.delete_at != 0
    }
}

/// The current user's membership in a channel (`server/public/model/channel_member.go`):
/// read state, counters and notification settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelMember {
    pub channel_id: ChannelId,
    pub user_id: UserId,
    pub roles: String,
    pub last_viewed_at: i64,
    pub msg_count: i64,
    pub mention_count: i64,
    pub mention_count_root: i64,
    pub urgent_mention_count: i64,
    pub msg_count_root: i64,
    /// `desktop`, `push`, `email`, `mark_unread` (`"mention"` = muted), ...; the value
    /// `"default"` means "use the user's setting".
    #[serde(deserialize_with = "string_map")]
    pub notify_props: HashMap<String, String>,
    pub last_update_at: i64,
    pub scheme_guest: bool,
    pub scheme_user: bool,
    pub scheme_admin: bool,
    pub explicit_roles: String,
}

impl ChannelMember {
    pub fn is_muted(&self) -> bool {
        self.notify_props.get("mark_unread").map(String::as_str) == Some("mention")
    }
}

/// A sidebar category with its channels (`server/public/model/channel_sidebar.go`).
/// `category_type` is `custom`, `channels`, `direct_messages` or `favorites`; `sorting` is
/// `""` (default), `alpha`, `recent` or `manual`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarCategory {
    pub id: CategoryId,
    pub user_id: UserId,
    pub team_id: TeamId,
    pub sort_order: i64,
    pub sorting: String,
    #[serde(rename = "type")]
    pub category_type: String,
    pub display_name: String,
    pub muted: bool,
    pub collapsed: bool,
    #[serde(deserialize_with = "null_default")]
    pub channel_ids: Vec<ChannelId>,
}

/// `GET /users/{uid}/teams/{tid}/channels/categories`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrderedSidebarCategories {
    #[serde(deserialize_with = "null_default")]
    pub categories: Vec<SidebarCategory>,
    #[serde(deserialize_with = "null_default")]
    pub order: Vec<CategoryId>,
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
