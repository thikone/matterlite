// SPDX-License-Identifier: AGPL-3.0-only

//! What changed in the store, for the UI copy (MatterLite.md §6.2) and the cache writer.

use mm_model::{ChannelId, PostId, TeamId, UserId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreDelta {
    /// Posts were added or changed (content, reactions, reply counts).
    PostsChanged(Vec<PostId>),
    /// Posts were removed: deleted, or a pending post replaced by the server's copy.
    PostsRemoved(Vec<PostId>),
    /// A channel's timeline (order or ranges) changed.
    TimelineChanged(ChannelId),
    /// A thread's replies changed.
    ThreadChanged(PostId),
    /// A channel's metadata or counters changed.
    ChannelChanged(ChannelId),
    /// My membership changed (counters, `last_viewed_at`, notify props).
    MemberChanged(ChannelId),
    /// I left or was removed from the channel, or it was deleted for me.
    ChannelRemoved(ChannelId),
    /// The channel was marked read locally. `notify_server`: the engine must also call
    /// `POST /channels/members/me/view` (a post arrived in the channel being viewed).
    MarkedRead {
        channel: ChannelId,
        notify_server: bool,
    },
    TeamChanged(TeamId),
    TeamRemoved(TeamId),
    /// Followed-thread counts of a team changed (CRT).
    ThreadCountsChanged(TeamId),
    UserChanged(UserId),
    StatusChanged(UserId),
    Typing {
        channel: ChannelId,
        /// The thread's root, empty for the channel.
        root: PostId,
        user: UserId,
    },
    PreferencesChanged,
    /// Collapsed Reply Threads turned on or off: timelines must be reloaded, since they hold
    /// root posts only under CRT.
    CrtChanged,
    /// The name display format changed: every name must be redrawn.
    NamesChanged,
    CategoriesChanged(TeamId),
    RolesChanged,
    ConfigChanged,
    LicenseChanged,
    EmojiChanged,
    /// A pending post was sent, failed or retried.
    PendingChanged(String),
    /// The store lacks data an event refers to; the engine fetches it.
    Fetch(Fetch),
}

/// Data the engine has to fetch for the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// A channel and my membership (new DM, added to a channel, channel restored).
    Channel(ChannelId),
    Team(TeamId),
    /// All sidebar categories of a team.
    Categories(TeamId),
    /// My roles changed: refetch them by name.
    Roles,
    Users(Vec<UserId>),
}
