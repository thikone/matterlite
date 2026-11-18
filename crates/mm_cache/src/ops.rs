// SPDX-License-Identifier: AGPL-3.0-only

//! What the writer thread writes, and how store changes become writes.
//!
//! The sync engine applies an event or a response to the [`Store`], gets [`StoreDelta`]s back
//! and turns them into [`Op`]s with [`ops_for`]: each op carries a copy of the changed entity,
//! so the writer never reads the store.

use mm_model::{
    Channel, ChannelId, ChannelMember, Emoji, Post, PostId, Role, SidebarCategory, Team, TeamId,
    TeamMember, ThreadResponse, User,
};
use mm_store::{Range, Store, StoreDelta, Timeline};

use crate::lifecycle::OutboxEntry;

/// Which timeline a set of ranges belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    Channel,
    Thread,
}

impl Owner {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Thread => "thread",
        }
    }
}

/// `kv` keys.
pub mod kv {
    /// My `User` (JSON); hydration needs it first.
    pub const ME: &str = "me";
    pub const CONFIG: &str = "config";
    pub const LICENSE: &str = "license";
    /// `HashMap<TeamId, ThreadCounts>` as JSON.
    pub const THREAD_COUNTS: &str = "thread_counts";
    /// The normalized server URL and the client config's `DiagnosticId` at the last sync, to
    /// notice a reinstalled or reset server (§8.7).
    pub const SERVER_URL: &str = "server_url";
    pub const DIAGNOSTIC_ID: &str = "diagnostic_id";
    /// `mm_ws::ResumeState` as JSON: resume the WebSocket after a restart.
    pub const WS_RESUME: &str = "ws_resume";
    /// The last time (ms) the cache was known to be in sync with the server: resyncs fetch
    /// changes since then.
    pub const SYNCED_THROUGH: &str = "synced_through";
    /// When profiles were last refreshed (`POST /users/ids?since=`).
    pub const USERS_SYNCED_AT: &str = "users_synced_at";
    /// When the account was last opened (ms), for the stored-accounts list.
    pub const LAST_USED: &str = "last_used";
    pub const LAST_TEAM: &str = "last_team";
    pub const LAST_CHANNEL: &str = "last_channel";
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    PutUser(User),
    PutTeam(Team),
    DeleteTeam(TeamId),
    PutTeamMember(TeamMember),
    DeleteTeamMember(TeamId),
    PutChannel(Channel),
    PutChannelMember(ChannelMember),
    /// The channel and its posts stay as a former chat (§8.7); only the membership goes.
    DeleteChannelMember(ChannelId),
    /// Memberships not in the list are deleted (after a full channel list); those channels
    /// become former chats.
    KeepChannelMembers(Vec<ChannelId>),
    PutOutbox(OutboxEntry),
    DeleteOutbox(String),
    /// Removes a channel and everything cached for it (a former chat past its retention).
    PurgeChannel(ChannelId),
    /// Team memberships not in the list are deleted.
    KeepTeams(Vec<TeamId>),
    /// Also updates the search index.
    PutPost(Post),
    /// An explicit deletion (author, admin): the post leaves the cache and the index.
    DeletePost(PostId),
    SetRanges {
        owner: Owner,
        id: String,
        ranges: Vec<Range>,
        empty: bool,
    },
    DeleteRanges {
        owner: Owner,
        id: String,
    },
    ReplacePreferences(Vec<(String, String, String)>),
    PutCategories(TeamId, Vec<SidebarCategory>),
    DeleteCategories(TeamId),
    ReplaceRoles(Vec<Role>),
    PutEmoji(Emoji),
    PutThread(ThreadResponse),
    DeleteThread(PostId),
    /// `None` deletes the key.
    Kv(String, Option<String>),
    /// The unsent text of a chat's (or a thread's) composer.
    PutDraft {
        channel: ChannelId,
        /// Empty for the channel's composer.
        root: PostId,
        message: String,
        updated_at: i64,
    },
    DeleteDraft {
        channel: ChannelId,
        root: PostId,
    },
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("entities serialize")
}

/// The writes that save everything in the store.
pub fn snapshot_ops(store: &Store) -> Vec<Op> {
    let mut ops = metadata_ops(store);
    ops.extend(
        store
            .posts
            .values()
            .filter(|post| !store.pending.contains_key(post.id.as_str()))
            .map(|post| Op::PutPost(post.as_ref().clone())),
    );
    for (channel, timeline) in &store.timelines {
        ops.push(ranges_op(Owner::Channel, channel.to_string(), timeline));
    }
    for (root, timeline) in &store.threads {
        ops.push(ranges_op(Owner::Thread, root.to_string(), timeline));
    }
    ops
}

/// Everything but posts, timelines and threads: what a bootstrap replaces (posts are written
/// as they're fetched).
pub fn metadata_ops(store: &Store) -> Vec<Op> {
    let mut ops = Vec::new();
    if let Some(me) = store.users.get(&store.me) {
        ops.push(Op::Kv(kv::ME.into(), Some(json(me.as_ref()))));
    }
    ops.push(Op::Kv(kv::CONFIG.into(), Some(json(&store.config))));
    ops.push(Op::Kv(kv::LICENSE.into(), Some(json(&store.license))));
    ops.push(thread_counts(store));
    ops.extend(
        store
            .users
            .values()
            .map(|user| Op::PutUser(user.as_ref().clone())),
    );
    ops.push(Op::KeepTeams(store.team_members.keys().cloned().collect()));
    ops.extend(
        store
            .teams
            .values()
            .map(|team| Op::PutTeam(team.as_ref().clone())),
    );
    ops.extend(store.team_members.values().cloned().map(Op::PutTeamMember));
    ops.push(Op::KeepChannelMembers(
        store.channel_members.keys().cloned().collect(),
    ));
    ops.extend(
        store
            .channels
            .values()
            .map(|channel| Op::PutChannel(channel.as_ref().clone())),
    );
    ops.extend(
        store
            .channel_members
            .values()
            .cloned()
            .map(Op::PutChannelMember),
    );
    ops.push(preferences(store));
    for (team, categories) in &store.categories {
        ops.push(Op::PutCategories(team.clone(), categories.clone()));
    }
    ops.push(Op::ReplaceRoles(store.roles.values().cloned().collect()));
    ops.extend(store.emoji.values().cloned().map(Op::PutEmoji));
    ops.extend(store.thread_states.values().cloned().map(Op::PutThread));
    ops
}

/// The writes for a batch of store changes.
pub fn ops_for(store: &Store, deltas: &[StoreDelta]) -> Vec<Op> {
    let mut ops = Vec::new();
    for delta in deltas {
        match delta {
            StoreDelta::PostsChanged(ids) => {
                for id in ids {
                    // Pending posts belong to the outbox, not the cache.
                    if store.pending.contains_key(id.as_str()) {
                        continue;
                    }
                    if let Some(post) = store.posts.get(id) {
                        ops.push(Op::PutPost(post.as_ref().clone()));
                    }
                }
            }
            StoreDelta::PostsRemoved(ids) => {
                for id in ids {
                    // A pending post replaced by the server's copy was never saved; deleting
                    // it is harmless.
                    if !store.posts.contains_key(id) {
                        ops.push(Op::DeletePost(id.clone()));
                        ops.push(Op::DeleteRanges {
                            owner: Owner::Thread,
                            id: id.to_string(),
                        });
                    }
                }
            }
            StoreDelta::TimelineChanged(channel) => match store.timelines.get(channel) {
                Some(timeline) => {
                    ops.push(ranges_op(Owner::Channel, channel.to_string(), timeline))
                }
                None => ops.push(Op::DeleteRanges {
                    owner: Owner::Channel,
                    id: channel.to_string(),
                }),
            },
            StoreDelta::ThreadChanged(root) => {
                if let Some(timeline) = store.threads.get(root) {
                    ops.push(ranges_op(Owner::Thread, root.to_string(), timeline));
                }
                match store.thread_states.get(root) {
                    Some(thread) => ops.push(Op::PutThread(thread.clone())),
                    None => ops.push(Op::DeleteThread(root.clone())),
                }
            }
            StoreDelta::ChannelChanged(channel) => {
                if let Some(channel) = store.channels.get(channel) {
                    ops.push(Op::PutChannel(channel.as_ref().clone()));
                }
            }
            StoreDelta::MemberChanged(channel) => {
                if let Some(member) = store.channel_members.get(channel) {
                    ops.push(Op::PutChannelMember(member.clone()));
                }
            }
            StoreDelta::ChannelRemoved(channel) => {
                ops.push(Op::DeleteChannelMember(channel.clone()));
                ops.push(Op::DeleteRanges {
                    owner: Owner::Channel,
                    id: channel.to_string(),
                });
            }
            StoreDelta::TeamChanged(team) => {
                if let Some(team) = store.teams.get(team) {
                    ops.push(Op::PutTeam(team.as_ref().clone()));
                }
                if let Some(member) = store.team_members.get(team) {
                    ops.push(Op::PutTeamMember(member.clone()));
                }
            }
            StoreDelta::TeamRemoved(team) => {
                ops.push(Op::DeleteTeamMember(team.clone()));
                ops.push(Op::DeleteTeam(team.clone()));
                ops.push(Op::DeleteCategories(team.clone()));
            }
            StoreDelta::ThreadCountsChanged(_) => ops.push(thread_counts(store)),
            StoreDelta::UserChanged(user) => {
                if let Some(user) = store.users.get(user) {
                    ops.push(Op::PutUser(user.as_ref().clone()));
                    if user.id == store.me {
                        ops.push(Op::Kv(kv::ME.into(), Some(json(user.as_ref()))));
                    }
                }
            }
            StoreDelta::PreferencesChanged => ops.push(preferences(store)),
            StoreDelta::CategoriesChanged(team) => match store.categories.get(team) {
                Some(categories) => ops.push(Op::PutCategories(team.clone(), categories.clone())),
                None => ops.push(Op::DeleteCategories(team.clone())),
            },
            StoreDelta::RolesChanged => {
                ops.push(Op::ReplaceRoles(store.roles.values().cloned().collect()));
            }
            StoreDelta::ConfigChanged => {
                ops.push(Op::Kv(kv::CONFIG.into(), Some(json(&store.config))));
            }
            StoreDelta::LicenseChanged => {
                ops.push(Op::Kv(kv::LICENSE.into(), Some(json(&store.license))));
            }
            StoreDelta::EmojiChanged => {
                ops.extend(store.emoji.values().cloned().map(Op::PutEmoji));
            }
            // Statuses and typing are live only (§8.7); read marks come with MemberChanged;
            // pending posts go to the outbox; the rest are signals for the UI or the engine.
            StoreDelta::StatusChanged(_)
            | StoreDelta::Typing { .. }
            | StoreDelta::MarkedRead { .. }
            | StoreDelta::CrtChanged
            | StoreDelta::NamesChanged
            | StoreDelta::PendingChanged(_)
            | StoreDelta::EditHistoryChanged(_)
            | StoreDelta::ActivityChanged
            | StoreDelta::Fetch(_) => {}
        }
    }
    ops
}

fn ranges_op(owner: Owner, id: String, timeline: &Timeline) -> Op {
    Op::SetRanges {
        owner,
        id,
        ranges: timeline.ranges().to_vec(),
        empty: timeline.is_known_empty(),
    }
}

fn thread_counts(store: &Store) -> Op {
    let counts: serde_json::Map<String, serde_json::Value> = store
        .thread_counts
        .iter()
        .map(|(team, counts)| {
            (
                team.to_string(),
                serde_json::json!([
                    counts.unread_threads,
                    counts.mentions,
                    counts.urgent_mentions
                ]),
            )
        })
        .collect();
    Op::Kv(
        kv::THREAD_COUNTS.into(),
        Some(serde_json::Value::Object(counts).to_string()),
    )
}

fn preferences(store: &Store) -> Op {
    Op::ReplacePreferences(
        store
            .preferences
            .iter()
            .map(|((category, name), value)| (category.clone(), name.clone(), value.clone()))
            .collect(),
    )
}
