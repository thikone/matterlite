// SPDX-License-Identifier: AGPL-3.0-only

//! WebSocket frames and events (WebApp.md §7).
//!
//! The server sends two kinds of JSON text frames: events (`{"event", "data", "broadcast",
//! "seq"}`) and responses to client requests (`{"status", "seq_reply", "data", "error"}`).
//! Many event payload fields are JSON documents encoded as strings; [`WsEvent::decode`] does
//! that second parse.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::channel::{Channel, ChannelMember, SidebarCategory};
use crate::config::ClientConfig;
use crate::de::{nested, null_default};
use crate::error::AppError;
use crate::ids::{CategoryId, ChannelId, PostId, TeamId, UserId};
use crate::post::{Emoji, Post, Reaction};
use crate::preference::Preference;
use crate::role::Role;
use crate::team::Team;
use crate::thread::ThreadResponse;
use crate::token::SessionToken;
use crate::user::User;

/// A frame received from the server.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerFrame {
    Event(EventFrame),
    Response(WsResponse),
}

impl ServerFrame {
    /// Parses a text frame. Frames with an `event` field are events; everything else is a
    /// response.
    pub fn parse(text: &str) -> serde_json::Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        if value.get("event").is_some() {
            let mut frame: EventFrame = serde_json::from_value(value)?;
            if frame.data.is_null() {
                frame.data = Value::Object(Map::new());
            }
            Ok(Self::Event(frame))
        } else {
            Ok(Self::Response(serde_json::from_value(value)?))
        }
    }
}

/// A server event before its payload is decoded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventFrame {
    pub event: String,
    /// Always an object after [`ServerFrame::parse`].
    pub data: Value,
    #[serde(deserialize_with = "null_default")]
    pub broadcast: Broadcast,
    /// Per-connection event sequence number, starting at 0 with `hello`.
    pub seq: i64,
}

/// Who an event was sent to. The channel or team an event concerns is usually here, not in
/// `data`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Broadcast {
    #[serde(deserialize_with = "null_default")]
    pub omit_users: HashMap<UserId, bool>,
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub team_id: TeamId,
    pub connection_id: String,
    pub omit_connection_id: String,
}

/// The reply to a client request, matched by `seq_reply`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WsResponse {
    /// `OK` or `FAIL`.
    pub status: String,
    pub seq_reply: i64,
    pub data: Option<Value>,
    pub error: Option<AppError>,
}

impl WsResponse {
    pub fn is_ok(&self) -> bool {
        self.status == "OK"
    }
}

/// A request the client sends (WebApp.md §7.6).
#[derive(Clone, Debug, PartialEq)]
pub enum ClientAction {
    AuthenticationChallenge(SessionToken),
    Ping,
    /// Typing in a channel (`parent_id` empty) or a thread.
    UserTyping {
        channel_id: ChannelId,
        parent_id: PostId,
    },
    /// What the user is looking at; the server uses it to decide about notifications.
    Presence(Presence),
    UserUpdateActiveStatus {
        user_is_active: bool,
        manual: bool,
    },
    GetStatuses,
    GetStatusesByIds(Vec<UserId>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Presence {
    Channel(ChannelId),
    Team(TeamId),
    /// A thread open in the threads view (`is_thread_view`) or beside a channel.
    Thread {
        channel_id: ChannelId,
        is_thread_view: bool,
    },
}

impl ClientAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::AuthenticationChallenge(_) => "authentication_challenge",
            Self::Ping => "ping",
            Self::UserTyping { .. } => "user_typing",
            Self::Presence(_) => "presence",
            Self::UserUpdateActiveStatus { .. } => "user_update_active_status",
            Self::GetStatuses => "get_statuses",
            Self::GetStatusesByIds(_) => "get_statuses_by_ids",
        }
    }

    fn data(&self) -> Option<Value> {
        match self {
            Self::AuthenticationChallenge(token) => Some(json!({ "token": token.expose() })),
            Self::Ping | Self::GetStatuses => None,
            Self::UserTyping {
                channel_id,
                parent_id,
            } => Some(json!({ "channel_id": channel_id, "parent_id": parent_id })),
            Self::Presence(Presence::Channel(channel_id)) => {
                Some(json!({ "channel_id": channel_id }))
            }
            Self::Presence(Presence::Team(team_id)) => Some(json!({ "team_id": team_id })),
            Self::Presence(Presence::Thread {
                channel_id,
                is_thread_view,
            }) => Some(json!({
                "thread_channel_id": channel_id,
                "is_thread_view": is_thread_view,
            })),
            Self::UserUpdateActiveStatus {
                user_is_active,
                manual,
            } => Some(json!({ "user_is_active": user_is_active, "manual": manual })),
            Self::GetStatusesByIds(user_ids) => Some(json!({ "user_ids": user_ids })),
        }
    }

    /// The JSON text frame for this request with client sequence number `seq` (starts at 1
    /// on every connection).
    pub fn to_frame(&self, seq: i64) -> String {
        let mut frame = json!({ "action": self.name(), "seq": seq });
        if let Some(data) = self.data() {
            frame["data"] = data;
        }
        frame.to_string()
    }
}

/// A decoded server event. Events MatterLite doesn't handle yet are [`WsEvent::Unknown`]; the
/// list grows with every server release, so unknown events are not errors.
#[derive(Clone, Debug, PartialEq)]
pub enum WsEvent {
    Hello(Hello),
    Posted(Box<Posted>),
    PostEdited(Box<Post>),
    PostDeleted(Box<PostDeleted>),
    EphemeralMessage(Box<Post>),
    PostUnread(PostUnread),
    ReactionAdded(Reaction),
    ReactionRemoved(Reaction),
    /// Someone types in `broadcast.channel_id`.
    Typing(Typing),
    StatusChange(StatusChange),
    /// Channels were viewed on another device or tab: mark them read.
    MultipleChannelsViewed(MultipleChannelsViewed),
    ChannelCreated(ChannelRef),
    ChannelUpdated(Box<Channel>),
    ChannelDeleted(ChannelDeleted),
    ChannelRestored(ChannelRef),
    ChannelConverted(ChannelRef),
    ChannelMemberUpdated(Box<ChannelMember>),
    /// A user joined `broadcast.channel_id`.
    UserAdded(TeamUserRef),
    UserRemoved(UserRemoved),
    DirectAdded(DirectAdded),
    GroupAdded(GroupAdded),
    AddedToTeam(TeamUserRef),
    LeaveTeam(TeamUserRef),
    UpdateTeam(Box<Team>),
    DeleteTeam(Box<Team>),
    RestoreTeam(Box<Team>),
    NewUser(UserRef),
    UserUpdated(Box<User>),
    UserRoleUpdated(UserRoleUpdated),
    /// `preference_changed` (one) and `preferences_changed` (several).
    PreferencesChanged(Vec<Preference>),
    PreferencesDeleted(Vec<Preference>),
    ThreadUpdated(Box<ThreadUpdated>),
    ThreadReadChanged(ThreadReadChanged),
    ThreadFollowChanged(ThreadFollowChanged),
    SidebarCategoryCreated(CategoryRef),
    SidebarCategoryUpdated(Vec<SidebarCategory>),
    SidebarCategoryDeleted(CategoryRef),
    SidebarCategoryOrderUpdated(CategoryOrder),
    EmojiAdded(Emoji),
    RoleUpdated(Box<Role>),
    ConfigChanged(ClientConfig),
    LicenseChanged(ClientConfig),
    Unknown(String),
}

/// Why an event payload could not be decoded.
#[derive(Debug, thiserror::Error)]
#[error("cannot decode `{event}` event: {source}")]
pub struct DecodeError {
    pub event: String,
    #[source]
    pub source: serde_json::Error,
}

impl WsEvent {
    pub fn decode(frame: &EventFrame) -> Result<Self, DecodeError> {
        Self::decode_data(&frame.event, &frame.data).map_err(|source| DecodeError {
            event: frame.event.clone(),
            source,
        })
    }

    fn decode_data(event: &str, data: &Value) -> serde_json::Result<Self> {
        Ok(match event {
            "hello" => Self::Hello(payload(data)?),
            "posted" => Self::Posted(payload(data)?),
            "post_edited" => Self::PostEdited(Box::new(field::<PostField>(data)?.post)),
            "post_deleted" => Self::PostDeleted(payload(data)?),
            "ephemeral_message" => Self::EphemeralMessage(Box::new(field::<PostField>(data)?.post)),
            "post_unread" => Self::PostUnread(payload(data)?),
            "reaction_added" => Self::ReactionAdded(field::<ReactionField>(data)?.reaction),
            "reaction_removed" => Self::ReactionRemoved(field::<ReactionField>(data)?.reaction),
            "typing" => Self::Typing(payload(data)?),
            "status_change" => Self::StatusChange(payload(data)?),
            "multiple_channels_viewed" => Self::MultipleChannelsViewed(payload(data)?),
            "channel_created" => Self::ChannelCreated(payload(data)?),
            "channel_updated" => {
                Self::ChannelUpdated(Box::new(field::<ChannelField>(data)?.channel))
            }
            "channel_deleted" => Self::ChannelDeleted(payload(data)?),
            "channel_restored" => Self::ChannelRestored(payload(data)?),
            "channel_converted" => Self::ChannelConverted(payload(data)?),
            "channel_member_updated" => Self::ChannelMemberUpdated(Box::new(
                field::<ChannelMemberField>(data)?.channel_member,
            )),
            "user_added" => Self::UserAdded(payload(data)?),
            "user_removed" => Self::UserRemoved(payload(data)?),
            "direct_added" => Self::DirectAdded(payload(data)?),
            "group_added" => Self::GroupAdded(payload(data)?),
            "added_to_team" => Self::AddedToTeam(payload(data)?),
            "leave_team" => Self::LeaveTeam(payload(data)?),
            "update_team" => Self::UpdateTeam(Box::new(field::<TeamField>(data)?.team)),
            "delete_team" => Self::DeleteTeam(Box::new(field::<TeamField>(data)?.team)),
            "restore_team" => Self::RestoreTeam(Box::new(field::<TeamField>(data)?.team)),
            "new_user" => Self::NewUser(payload(data)?),
            "user_updated" => Self::UserUpdated(Box::new(field::<UserField>(data)?.user)),
            "user_role_updated" => Self::UserRoleUpdated(payload(data)?),
            "preference_changed" => {
                Self::PreferencesChanged(vec![field::<PreferenceField>(data)?.preference])
            }
            "preferences_changed" => {
                Self::PreferencesChanged(field::<PreferencesField>(data)?.preferences)
            }
            "preferences_deleted" => {
                Self::PreferencesDeleted(field::<PreferencesField>(data)?.preferences)
            }
            "thread_updated" => Self::ThreadUpdated(payload(data)?),
            "thread_read_changed" => Self::ThreadReadChanged(payload(data)?),
            "thread_follow_changed" => Self::ThreadFollowChanged(payload(data)?),
            "sidebar_category_created" => Self::SidebarCategoryCreated(payload(data)?),
            "sidebar_category_updated" => Self::SidebarCategoryUpdated(
                field::<UpdatedCategoriesField>(data)?.updated_categories,
            ),
            "sidebar_category_deleted" => Self::SidebarCategoryDeleted(payload(data)?),
            "sidebar_category_order_updated" => Self::SidebarCategoryOrderUpdated(payload(data)?),
            "emoji_added" => Self::EmojiAdded(field::<EmojiField>(data)?.emoji),
            "role_updated" => Self::RoleUpdated(Box::new(field::<RoleField>(data)?.role)),
            "config_changed" => Self::ConfigChanged(field::<ConfigField>(data)?.config),
            "license_changed" => Self::LicenseChanged(field::<LicenseField>(data)?.license),
            other => Self::Unknown(other.to_owned()),
        })
    }
}

fn payload<T: DeserializeOwned>(data: &Value) -> serde_json::Result<T> {
    T::deserialize(data)
}

/// Decodes a wrapper whose single field is required: a missing or empty entity is an error,
/// not a default value.
fn field<T: DeserializeOwned + Required>(data: &Value) -> serde_json::Result<T> {
    let wrapper = T::deserialize(data)?;
    if wrapper.is_missing() {
        return Err(serde::de::Error::custom(format!("missing `{}`", T::FIELD)));
    }
    Ok(wrapper)
}

trait Required {
    const FIELD: &'static str;
    fn is_missing(&self) -> bool;
}

macro_rules! required_field {
    ($wrapper:ident, $field:ident: $ty:ty, $wire:literal, |$value:ident| $missing:expr) => {
        #[derive(Deserialize)]
        struct $wrapper {
            #[serde(rename = $wire, default, deserialize_with = "nested")]
            $field: $ty,
        }

        impl Required for $wrapper {
            const FIELD: &'static str = $wire;
            fn is_missing(&self) -> bool {
                let $value = &self.$field;
                $missing
            }
        }
    };
}

required_field!(PostField, post: Post, "post", |post| post.id.is_empty());
required_field!(ReactionField, reaction: Reaction, "reaction", |reaction| reaction
    .post_id
    .is_empty());
required_field!(ChannelField, channel: Channel, "channel", |channel| channel.id.is_empty());
required_field!(
    ChannelMemberField,
    channel_member: ChannelMember,
    "channelMember",
    |member| member.channel_id.is_empty()
);
required_field!(TeamField, team: Team, "team", |team| team.id.is_empty());
required_field!(UserField, user: User, "user", |user| user.id.is_empty());
required_field!(PreferenceField, preference: Preference, "preference", |preference| preference
    .category
    .is_empty());
required_field!(PreferencesField, preferences: Vec<Preference>, "preferences", |_preferences| {
    false
});
required_field!(
    UpdatedCategoriesField,
    updated_categories: Vec<SidebarCategory>,
    "updatedCategories",
    |_categories| false
);
required_field!(EmojiField, emoji: Emoji, "emoji", |emoji| emoji.id.is_empty());
required_field!(RoleField, role: Role, "role", |role| role.name.is_empty());
required_field!(ConfigField, config: ClientConfig, "config", |_config| false);
required_field!(LicenseField, license: ClientConfig, "license", |_license| false);

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Hello {
    pub connection_id: String,
    /// `<version>.<build>.<config hash>.<is enterprise>`; a change after a reconnect means the
    /// server restarted or was upgraded.
    pub server_version: String,
    pub server_hostname: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Posted {
    #[serde(deserialize_with = "nested")]
    pub post: Post,
    pub channel_type: String,
    pub channel_name: String,
    pub channel_display_name: String,
    pub sender_name: String,
    pub team_id: TeamId,
    pub set_online: bool,
    /// Users mentioned by the post (only sent to them).
    #[serde(deserialize_with = "nested")]
    pub mentions: Vec<UserId>,
    /// Thread followers (CRT).
    #[serde(deserialize_with = "nested")]
    pub followers: Vec<UserId>,
    /// The server wants a `posted_notify_ack` for this event.
    pub should_ack: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PostDeleted {
    #[serde(deserialize_with = "nested")]
    pub post: Post,
    /// Only in the copy sent to admins.
    pub delete_by: Option<UserId>,
}

/// The user marked a post unread (on any device): the channel's counters are reset to these.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PostUnread {
    pub channel_id: ChannelId,
    pub team_id: TeamId,
    pub post_id: PostId,
    pub msg_count: i64,
    pub msg_count_root: i64,
    pub mention_count: i64,
    pub mention_count_root: i64,
    pub urgent_mention_count: i64,
    pub last_viewed_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Typing {
    pub user_id: UserId,
    /// The thread's root post, empty when typing in the channel.
    pub parent_id: PostId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StatusChange {
    pub user_id: UserId,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MultipleChannelsViewed {
    #[serde(deserialize_with = "null_default")]
    pub channel_times: HashMap<ChannelId, i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChannelRef {
    pub channel_id: ChannelId,
    pub team_id: TeamId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ChannelDeleted {
    pub channel_id: ChannelId,
    pub delete_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TeamUserRef {
    pub team_id: TeamId,
    pub user_id: UserId,
}

/// Sent to the channel (with `user_id`) and to the removed user (with `channel_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserRemoved {
    pub user_id: UserId,
    pub channel_id: ChannelId,
    pub remover_id: UserId,
}

/// A DM channel (`broadcast.channel_id`) was created.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DirectAdded {
    pub creator_id: UserId,
    pub teammate_id: UserId,
}

/// A GM channel (`broadcast.channel_id`) was created.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GroupAdded {
    #[serde(deserialize_with = "nested")]
    pub teammate_ids: Vec<UserId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserRef {
    pub user_id: UserId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UserRoleUpdated {
    pub user_id: UserId,
    pub roles: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ThreadUpdated {
    #[serde(deserialize_with = "nested")]
    pub thread: ThreadResponse,
    pub previous_unread_mentions: i64,
    pub previous_unread_replies: i64,
}

/// A thread's read state changed. Without `thread_id`, all threads of the team were marked
/// read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ThreadReadChanged {
    pub thread_id: PostId,
    pub channel_id: ChannelId,
    pub timestamp: i64,
    pub unread_mentions: i64,
    pub unread_replies: i64,
    pub previous_unread_mentions: i64,
    pub previous_unread_replies: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ThreadFollowChanged {
    pub thread_id: PostId,
    pub state: bool,
    pub reply_count: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CategoryRef {
    pub category_id: CategoryId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CategoryOrder {
    #[serde(deserialize_with = "nested")]
    pub order: Vec<CategoryId>,
}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod tests;
