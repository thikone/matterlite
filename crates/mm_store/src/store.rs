// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mm_model::preference::keys;
use mm_model::{
    Channel, ChannelId, ChannelMember, ChannelType, ClientConfig, CollapsedThreads, Emoji,
    OrderedSidebarCategories, Post, PostId, PostList, Preference, Role, SidebarCategory, Status,
    Team, TeamId, TeamMember, ThreadList, ThreadResponse, User, UserId,
};

use crate::delta::StoreDelta;
use crate::names::{NameFormat, dm_partner};
use crate::timeline::{Key, Page, Timeline};
use crate::unread::{ChannelUnread, TeamUnread};

/// Followed-thread counts of a team (`/users/me/teams/unread`, thread events).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadCounts {
    /// Threads with unread replies.
    pub unread_threads: i64,
    pub mentions: i64,
    pub urgent_mentions: i64,
}

/// What a posts request asked for, so the page joins the right timeline range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageRequest {
    /// The newest page (`GET /channels/{id}/posts`).
    Newest,
    /// `before=<id>`: older than a loaded post.
    Before(PostId),
    /// `after=<id>`: newer than a loaded post.
    After(PostId),
    /// The window around the first unread post (`/posts/unread`).
    AroundUnread,
    /// `since=<ms>`: everything created or changed since then. `complete` is false when the
    /// reply hit the server's cap of 1000 posts (MatterLite.md §8.7), so the posts can't be
    /// assumed contiguous with what's loaded.
    Since { complete: bool },
}

/// A post being sent (WebApp.md §9.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPost {
    pub channel: ChannelId,
    pub root: PostId,
    pub failed: bool,
}

/// The normalized state of one account on one server (MatterLite.md §8.1). The sync engine
/// owns it; events go through [`Store::apply`](crate::Store::apply) and each change is
/// reported as [`StoreDelta`]s.
#[derive(Clone, Debug, Default)]
pub struct Store {
    pub me: UserId,
    pub config: ClientConfig,
    pub license: ClientConfig,
    pub users: HashMap<UserId, Arc<User>>,
    pub statuses: HashMap<UserId, Status>,
    pub teams: HashMap<TeamId, Arc<Team>>,
    pub team_members: HashMap<TeamId, TeamMember>,
    pub thread_counts: HashMap<TeamId, ThreadCounts>,
    pub channels: HashMap<ChannelId, Arc<Channel>>,
    /// My memberships.
    pub channel_members: HashMap<ChannelId, ChannelMember>,
    pub posts: HashMap<PostId, Arc<Post>>,
    /// Per channel; under CRT root posts only.
    pub timelines: HashMap<ChannelId, Timeline>,
    /// Replies per root post.
    pub threads: HashMap<PostId, Timeline>,
    /// Followed threads (CRT), by root post id.
    pub thread_states: HashMap<PostId, ThreadResponse>,
    pub preferences: HashMap<(String, String), String>,
    /// Sidebar categories per team, in display order.
    pub categories: HashMap<TeamId, Vec<SidebarCategory>>,
    pub roles: HashMap<String, Role>,
    /// Custom emoji by name.
    pub emoji: HashMap<String, Emoji>,
    /// Pending posts by `pending_post_id` (also their id in `posts` until confirmed).
    pub pending: HashMap<String, PendingPost>,
    /// The channel on screen in a focused window: new posts there mark it read.
    pub viewing: Option<ChannelId>,
    /// Channels marked unread by hand: viewing them doesn't mark them read again until they
    /// are left (`manuallyUnread`).
    pub manually_unread: HashSet<ChannelId>,
}

impl Store {
    pub fn new(me: User) -> Self {
        let mut store = Self {
            me: me.id.clone(),
            ..Self::default()
        };
        store.users.insert(me.id.clone(), Arc::new(me));
        store
    }

    // ---- loading (bootstrap and REST responses) ----

    pub fn set_config(&mut self, config: ClientConfig) {
        self.config = config;
    }

    pub fn set_license(&mut self, license: ClientConfig) {
        self.license = license;
    }

    /// Replaces all preferences (bootstrap).
    pub fn set_preferences(&mut self, preferences: Vec<Preference>) {
        self.preferences = preferences
            .into_iter()
            .map(|preference| ((preference.category, preference.name), preference.value))
            .collect();
    }

    pub fn set_teams(&mut self, teams: Vec<Team>, members: Vec<TeamMember>) {
        self.teams = teams
            .into_iter()
            .map(|team| (team.id.clone(), Arc::new(team)))
            .collect();
        self.team_members = members
            .into_iter()
            .filter(|member| member.delete_at == 0)
            .map(|member| (member.team_id.clone(), member))
            .collect();
    }

    /// Takes the thread counts of `/users/me/teams/unread?include_collapsed_threads=true`.
    /// The channel counts there are sums the store computes itself.
    pub fn set_team_unreads(&mut self, unreads: Vec<mm_model::TeamUnread>) {
        for unread in unreads {
            self.thread_counts.insert(
                unread.team_id,
                ThreadCounts {
                    unread_threads: unread.thread_count,
                    mentions: unread.thread_mention_count,
                    urgent_mentions: unread.thread_urgent_mention_count,
                },
            );
        }
    }

    /// Replaces my channels and memberships (bootstrap, resync). Timelines of channels that
    /// are gone are dropped; the engine keeps their cached data as "former chats".
    pub fn set_channels(&mut self, channels: Vec<Channel>, members: Vec<ChannelMember>) {
        let members: HashMap<ChannelId, ChannelMember> = members
            .into_iter()
            .filter(|member| member.user_id == self.me)
            .map(|member| (member.channel_id.clone(), member))
            .collect();
        self.channels = channels
            .into_iter()
            .filter(|channel| members.contains_key(&channel.id))
            .map(|channel| (channel.id.clone(), Arc::new(channel)))
            .collect();
        self.channel_members = members;
        self.timelines
            .retain(|channel, _| self.channel_members.contains_key(channel));
    }

    /// Adds or replaces one channel and my membership (a new DM, a channel I was added to).
    pub fn upsert_channel(&mut self, channel: Channel, member: ChannelMember) -> Vec<StoreDelta> {
        let id = channel.id.clone();
        self.channels.insert(id.clone(), Arc::new(channel));
        self.channel_members.insert(id.clone(), member);
        vec![
            StoreDelta::ChannelChanged(id.clone()),
            StoreDelta::MemberChanged(id),
        ]
    }

    pub fn set_categories(&mut self, team: TeamId, ordered: OrderedSidebarCategories) {
        let mut by_id: HashMap<_, _> = ordered
            .categories
            .into_iter()
            .map(|category| (category.id.clone(), category))
            .collect();
        let mut categories: Vec<SidebarCategory> = ordered
            .order
            .iter()
            .filter_map(|id| by_id.remove(id))
            .collect();
        // Categories missing from `order` go last, by sort order.
        let mut rest: Vec<_> = by_id.into_values().collect();
        rest.sort_by_key(|category| category.sort_order);
        categories.extend(rest);
        self.categories.insert(team, categories);
    }

    pub fn set_roles(&mut self, roles: Vec<Role>) {
        self.roles = roles
            .into_iter()
            .map(|role| (role.name.clone(), role))
            .collect();
    }

    pub fn add_users(&mut self, users: Vec<User>) -> Vec<StoreDelta> {
        users
            .into_iter()
            .map(|user| {
                let id = user.id.clone();
                self.users.insert(id.clone(), Arc::new(user));
                StoreDelta::UserChanged(id)
            })
            .collect()
    }

    pub fn set_statuses(&mut self, statuses: Vec<Status>) -> Vec<StoreDelta> {
        statuses
            .into_iter()
            .map(|status| {
                let id = status.user_id.clone();
                self.statuses.insert(id.clone(), status);
                StoreDelta::StatusChanged(id)
            })
            .collect()
    }

    pub fn add_emoji(&mut self, emoji: Vec<Emoji>) {
        for emoji in emoji {
            self.emoji.insert(emoji.name.clone(), emoji);
        }
    }

    /// Adds a page of a channel's posts. Under CRT the server returns root posts only (and
    /// the roots of included replies in `posts`); replies found in the page also go to their
    /// thread.
    pub fn add_posts(
        &mut self,
        channel: &ChannelId,
        list: &PostList,
        request: PageRequest,
    ) -> Vec<StoreDelta> {
        let mut changed = Vec::new();
        for post in list.posts.values() {
            changed.push(post.id.clone());
            self.posts.insert(post.id.clone(), Arc::new(post.clone()));
        }
        let crt = self.crt_enabled();
        let keys: Vec<Key> = list
            .ordered()
            .filter(|post| post.channel_id == *channel && !post.is_deleted())
            .filter(|post| !crt || post.is_root())
            .map(|post| (post.create_at, post.id.clone()))
            .collect();
        for post in list.ordered().filter(|post| !post.is_root()) {
            self.threads
                .entry(post.root_id.clone())
                .or_default()
                .insert_live((post.create_at, post.id.clone()));
        }
        let timeline = self.timelines.entry(channel.clone()).or_default();
        let anchor = match &request {
            PageRequest::Before(id) | PageRequest::After(id) => Some(id),
            _ => None,
        };
        let adjacent: Vec<Key> = anchor
            .into_iter()
            .chain([&list.prev_post_id, &list.next_post_id])
            .filter_map(|id| timeline.key_of(id))
            .collect();
        match request {
            PageRequest::Since { complete } => {
                // Edits are already applied (the posts map). New posts join the timeline only
                // when the reply is complete; otherwise the engine loads the newest page next,
                // which leaves a hole to backfill.
                if complete {
                    timeline.insert_since(keys);
                }
                for post in list.posts.values().filter(|post| post.is_deleted()) {
                    timeline.remove(&post.id);
                }
            }
            PageRequest::Newest
            | PageRequest::Before(_)
            | PageRequest::After(_)
            | PageRequest::AroundUnread => {
                timeline.insert_page(Page {
                    keys,
                    reaches_newest: list.next_post_id.is_empty()
                        && !matches!(request, PageRequest::Before(_)),
                    reaches_start: list.prev_post_id.is_empty()
                        && !matches!(request, PageRequest::After(_)),
                    adjacent,
                });
            }
        }
        vec![
            StoreDelta::PostsChanged(changed),
            StoreDelta::TimelineChanged(channel.clone()),
        ]
    }

    /// Adds a whole thread (`GET /posts/{root}/thread`).
    pub fn add_thread(&mut self, root: &PostId, list: &PostList) -> Vec<StoreDelta> {
        let mut changed = Vec::new();
        let mut replies = Timeline::new();
        let keys = list
            .posts
            .values()
            .filter(|post| post.root_id == *root && !post.is_deleted())
            .map(|post| (post.create_at, post.id.clone()))
            .collect();
        replies.insert_page(Page {
            keys,
            reaches_newest: true,
            reaches_start: true,
            adjacent: Vec::new(),
        });
        for post in list.posts.values() {
            changed.push(post.id.clone());
            self.posts.insert(post.id.clone(), Arc::new(post.clone()));
        }
        self.threads.insert(root.clone(), replies);
        vec![
            StoreDelta::PostsChanged(changed),
            StoreDelta::ThreadChanged(root.clone()),
        ]
    }

    /// Followed threads of a team (`GET /users/{id}/teams/{team}/threads`): their states and
    /// the team's totals.
    pub fn set_threads(&mut self, team: &TeamId, list: ThreadList) -> Vec<StoreDelta> {
        self.thread_counts.insert(
            team.clone(),
            ThreadCounts {
                unread_threads: list.total_unread_threads,
                mentions: list.total_unread_mentions,
                urgent_mentions: list.total_unread_urgent_mentions,
            },
        );
        let mut changed = Vec::new();
        for mut thread in list.threads {
            if let Some(post) = thread.post.take() {
                changed.push(post.id.clone());
                self.posts.insert(post.id.clone(), Arc::new(post));
            }
            self.thread_states.insert(thread.id.clone(), thread);
        }
        vec![
            StoreDelta::PostsChanged(changed),
            StoreDelta::ThreadCountsChanged(team.clone()),
        ]
    }

    // ---- derived views ----

    pub fn preference(&self, category: &str, name: &str) -> Option<&str> {
        self.preferences
            .get(&(category.to_owned(), name.to_owned()))
            .map(String::as_str)
    }

    /// Collapsed Reply Threads (WebApp.md §10.3).
    pub fn crt_enabled(&self) -> bool {
        self.config
            .collapsed_threads()
            .is_enabled(self.preference(keys::DISPLAY_SETTINGS, keys::COLLAPSED_REPLY_THREADS))
    }

    pub fn crt_setting(&self) -> CollapsedThreads {
        self.config.collapsed_threads()
    }

    pub fn channel_unread(&self, channel: &ChannelId) -> Option<ChannelUnread> {
        Some(ChannelUnread::compute(
            self.channels.get(channel)?,
            self.channel_members.get(channel)?,
            self.crt_enabled(),
        ))
    }

    /// A team's badge: its non-archived channels, plus followed threads under CRT.
    pub fn team_unread(&self, team: &TeamId) -> TeamUnread {
        let mut unread = self.sum_channels(|channel| channel.team_id == *team);
        if self.crt_enabled()
            && let Some(threads) = self.thread_counts.get(team)
        {
            unread.threads = threads.unread_threads;
            unread.thread_mentions = threads.mentions;
            unread.thread_urgent_mentions = threads.urgent_mentions;
        }
        unread
    }

    /// DMs and GMs (they belong to no team and count on every team's view).
    pub fn direct_unread(&self) -> TeamUnread {
        self.sum_channels(|channel| channel.channel_type.is_direct_or_group())
    }

    fn sum_channels(&self, filter: impl Fn(&Channel) -> bool) -> TeamUnread {
        let crt = self.crt_enabled();
        let mut sum = TeamUnread::default();
        for (id, member) in &self.channel_members {
            if let Some(channel) = self.channels.get(id)
                && !channel.is_deleted()
                && filter(channel)
            {
                sum.add_channel(ChannelUnread::compute(channel, member, crt));
            }
        }
        sum
    }

    /// My channels in a team (or DMs/GMs for an empty team id), archived ones included.
    pub fn channels_in_team<'a>(
        &'a self,
        team: &'a TeamId,
    ) -> impl Iterator<Item = &'a Arc<Channel>> + 'a {
        self.channels.values().filter(move |channel| {
            channel.team_id == *team && self.channel_members.contains_key(&channel.id)
        })
    }

    /// The name format for other users (`getTeammateNameDisplaySetting`).
    pub fn name_format(&self) -> NameFormat {
        let locked = self.license.bool("LockTeammateNameDisplay")
            && self.config.bool("LockTeammateNameDisplay");
        match self.preference(keys::DISPLAY_SETTINGS, keys::NAME_FORMAT) {
            Some(value) if !locked => NameFormat::parse(value),
            _ => NameFormat::parse(self.config.get("TeammateNameDisplay").unwrap_or("")),
        }
    }

    /// A user's display name, or `None` when the profile isn't loaded.
    pub fn user_name(&self, user: &UserId) -> Option<String> {
        Some(self.name_format().display(self.users.get(user)?))
    }

    /// A channel's name for the sidebar and tabs: the partner for a DM (the self-DM shows my
    /// name), the members other than me for a GM, else the channel's display name.
    pub fn channel_name(&self, channel: &ChannelId) -> Option<String> {
        let channel = self.channels.get(channel)?;
        Some(match channel.channel_type {
            ChannelType::Direct => {
                let partner = dm_partner(&channel.name, &self.me)?;
                self.user_name(&partner)
                    .unwrap_or_else(|| partner.to_string())
            }
            ChannelType::Group => {
                // The server's display name lists every member's username, sorted.
                let my_username = self.users.get(&self.me).map(|me| me.username.as_str());
                let by_username: HashMap<&str, &User> = self
                    .users
                    .values()
                    .map(|user| (user.username.as_str(), user.as_ref()))
                    .collect();
                let format = self.name_format();
                channel
                    .display_name
                    .split(',')
                    .map(str::trim)
                    .filter(|username| !username.is_empty() && Some(*username) != my_username)
                    .map(|username| {
                        by_username
                            .get(username)
                            .map_or_else(|| username.to_owned(), |user| format.display(user))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
            _ => channel.display_name.clone(),
        })
    }

    /// The posts shown in a channel, newest first; `None` marks a hole.
    pub fn timeline_posts(&self, channel: &ChannelId) -> Vec<Option<&Arc<Post>>> {
        self.timelines
            .get(channel)
            .map_or_else(Vec::new, |timeline| {
                timeline
                    .items()
                    .into_iter()
                    .map(|id| id.and_then(|id| self.posts.get(id)))
                    .collect()
            })
    }

    /// Whether I may do something in a channel: system, team and channel roles together
    /// (`haveIChannelPermission`, WebApp.md §16). Roles must be loaded.
    pub fn has_channel_permission(&self, channel: &ChannelId, permission: &str) -> bool {
        let mut role_names: Vec<&str> = Vec::new();
        if let Some(me) = self.users.get(&self.me) {
            role_names.extend(me.role_names());
        }
        if let Some(channel) = self.channels.get(channel)
            && let Some(team) = self.team_members.get(&channel.team_id)
        {
            role_names.extend(team.roles.split_whitespace());
        }
        if let Some(member) = self.channel_members.get(channel) {
            role_names.extend(member.roles.split_whitespace());
        }
        role_names
            .iter()
            .filter_map(|name| self.roles.get(*name))
            .any(|role| role.has_permission(permission))
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
