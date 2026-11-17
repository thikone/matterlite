// SPDX-License-Identifier: AGPL-3.0-only

//! WebSocket events and own sends applied to the [`Store`] (WebApp.md §7.6, §9.3, §10.2).

use std::sync::Arc;

use mm_model::ws::{Broadcast, Posted, WsEvent};
use mm_model::{ChannelId, Post, PostId, Reaction, TeamId, UserId};

use crate::delta::{Fetch, StoreDelta};
use crate::store::{PendingPost, Store};
use crate::unread::{counts_as_message, is_notification_suppressed, is_own_message, is_urgent};

impl Store {
    /// Applies one event and reports what changed. Events for data the store doesn't have
    /// yet ask the engine to fetch it ([`StoreDelta::Fetch`]).
    pub fn apply(&mut self, event: &WsEvent, broadcast: &Broadcast) -> Vec<StoreDelta> {
        let mut deltas = Vec::new();
        match event {
            WsEvent::Posted(posted) => self.posted(posted, &mut deltas),
            WsEvent::PostEdited(post) => self.post_edited(post, &mut deltas),
            WsEvent::PostDeleted(deleted) => self.post_deleted(&deleted.post, &mut deltas),
            WsEvent::EphemeralMessage(post) => self.insert_post(post, &mut deltas),
            WsEvent::PostUnread(unread) => {
                // The server names the channel in the broadcast only.
                let channel = if unread.channel_id.is_empty() {
                    &broadcast.channel_id
                } else {
                    &unread.channel_id
                };
                if let Some(member) = self.channel_members.get_mut(channel) {
                    member.msg_count = unread.msg_count;
                    member.msg_count_root = unread.msg_count_root;
                    member.mention_count = unread.mention_count;
                    member.mention_count_root = unread.mention_count_root;
                    member.urgent_mention_count = unread.urgent_mention_count;
                    member.last_viewed_at = unread.last_viewed_at;
                    self.manually_unread.insert(channel.clone());
                    deltas.push(StoreDelta::MemberChanged(channel.clone()));
                }
            }
            WsEvent::ReactionAdded(reaction) => self.reaction(reaction, true, &mut deltas),
            WsEvent::ReactionRemoved(reaction) => self.reaction(reaction, false, &mut deltas),
            WsEvent::Typing(typing) => deltas.push(StoreDelta::Typing {
                channel: broadcast.channel_id.clone(),
                root: typing.parent_id.clone(),
                user: typing.user_id.clone(),
            }),
            WsEvent::StatusChange(change) => {
                let status = self.statuses.entry(change.user_id.clone()).or_default();
                status.user_id = change.user_id.clone();
                status.status = change.status.clone();
                deltas.push(StoreDelta::StatusChanged(change.user_id.clone()));
            }
            WsEvent::MultipleChannelsViewed(viewed) => {
                for (channel, at) in &viewed.channel_times {
                    self.mark_read_local(channel, *at, false, &mut deltas);
                }
            }
            WsEvent::ChannelCreated(reference)
            | WsEvent::ChannelRestored(reference)
            | WsEvent::ChannelConverted(reference) => {
                deltas.push(StoreDelta::Fetch(Fetch::Channel(
                    reference.channel_id.clone(),
                )));
            }
            WsEvent::ChannelUpdated(channel) => {
                if self.channel_members.contains_key(&channel.id) {
                    self.channels
                        .insert(channel.id.clone(), Arc::new(channel.as_ref().clone()));
                    deltas.push(StoreDelta::ChannelChanged(channel.id.clone()));
                }
            }
            WsEvent::ChannelDeleted(deleted) => {
                if let Some(channel) = self.channels.get_mut(&deleted.channel_id) {
                    Arc::make_mut(channel).delete_at = deleted.delete_at.max(1);
                    deltas.push(StoreDelta::ChannelChanged(deleted.channel_id.clone()));
                }
            }
            WsEvent::ChannelMemberUpdated(member) => {
                if member.user_id == self.me {
                    self.channel_members
                        .insert(member.channel_id.clone(), member.as_ref().clone());
                    deltas.push(StoreDelta::MemberChanged(member.channel_id.clone()));
                }
            }
            WsEvent::UserAdded(added) => {
                if added.user_id == self.me && !broadcast.channel_id.is_empty() {
                    deltas.push(StoreDelta::Fetch(Fetch::Channel(
                        broadcast.channel_id.clone(),
                    )));
                }
            }
            WsEvent::UserRemoved(removed) => {
                // Sent to the channel with `user_id`, and to the removed user with `channel_id`.
                let channel = if removed.channel_id.is_empty() {
                    broadcast.channel_id.clone()
                } else {
                    removed.channel_id.clone()
                };
                let me_removed = removed.user_id == self.me
                    || (removed.user_id.is_empty() && broadcast.user_id == self.me);
                if me_removed && self.channel_members.remove(&channel).is_some() {
                    self.channels.remove(&channel);
                    self.timelines.remove(&channel);
                    self.manually_unread.remove(&channel);
                    deltas.push(StoreDelta::ChannelRemoved(channel));
                }
            }
            WsEvent::DirectAdded(_) | WsEvent::GroupAdded(_) => {
                if !self.channel_members.contains_key(&broadcast.channel_id) {
                    deltas.push(StoreDelta::Fetch(Fetch::Channel(
                        broadcast.channel_id.clone(),
                    )));
                }
            }
            WsEvent::AddedToTeam(added) => {
                if added.user_id == self.me {
                    deltas.push(StoreDelta::Fetch(Fetch::Team(added.team_id.clone())));
                }
            }
            WsEvent::LeaveTeam(left) => {
                if left.user_id == self.me {
                    self.remove_team(&left.team_id, &mut deltas);
                }
            }
            WsEvent::UpdateTeam(team) | WsEvent::RestoreTeam(team) => {
                if self.team_members.contains_key(&team.id) {
                    self.teams
                        .insert(team.id.clone(), Arc::new(team.as_ref().clone()));
                    deltas.push(StoreDelta::TeamChanged(team.id.clone()));
                }
            }
            WsEvent::DeleteTeam(team) => self.remove_team(&team.id, &mut deltas),
            WsEvent::NewUser(_) => {}
            WsEvent::UserUpdated(user) => {
                // The event's copy is sanitized (no e-mail for others); keep fields it lacks.
                let mut user = user.as_ref().clone();
                if let Some(known) = self.users.get(&user.id)
                    && user.email.is_empty()
                {
                    user.email = known.email.clone();
                }
                self.users.insert(user.id.clone(), Arc::new(user.clone()));
                deltas.push(StoreDelta::UserChanged(user.id));
            }
            WsEvent::UserRoleUpdated(updated) => {
                if let Some(user) = self.users.get_mut(&updated.user_id) {
                    Arc::make_mut(user).roles = updated.roles.clone();
                    deltas.push(StoreDelta::UserChanged(updated.user_id.clone()));
                }
                if updated.user_id == self.me {
                    deltas.push(StoreDelta::Fetch(Fetch::Roles));
                }
            }
            WsEvent::PreferencesChanged(preferences) => {
                deltas.extend(self.put_preferences(preferences));
            }
            WsEvent::PreferencesDeleted(preferences) => {
                deltas.extend(self.remove_preferences(preferences));
            }
            WsEvent::ThreadUpdated(updated) => {
                let thread = &updated.thread;
                self.adjust_thread_counts(
                    &broadcast.team_id,
                    (
                        updated.previous_unread_replies,
                        updated.previous_unread_mentions,
                    ),
                    (thread.unread_replies, thread.unread_mentions),
                    &mut deltas,
                );
                let mut thread = thread.clone();
                if let Some(root) = self.put_thread_root(&mut thread) {
                    deltas.push(StoreDelta::PostsChanged(vec![root]));
                }
                deltas.push(StoreDelta::ThreadChanged(thread.id.clone()));
                self.thread_states.insert(thread.id.clone(), thread);
            }
            WsEvent::ThreadReadChanged(read) => {
                if read.thread_id.is_empty() {
                    // All threads of the team were marked read.
                    for state in self.thread_states.values_mut() {
                        state.unread_replies = 0;
                        state.unread_mentions = 0;
                        state.last_viewed_at = state.last_viewed_at.max(read.timestamp);
                    }
                    if self.thread_counts.remove(&broadcast.team_id).is_some() {
                        deltas.push(StoreDelta::ThreadCountsChanged(broadcast.team_id.clone()));
                    }
                } else {
                    self.adjust_thread_counts(
                        &broadcast.team_id,
                        (read.previous_unread_replies, read.previous_unread_mentions),
                        (read.unread_replies, read.unread_mentions),
                        &mut deltas,
                    );
                    if let Some(state) = self.thread_states.get_mut(&read.thread_id) {
                        state.unread_replies = read.unread_replies;
                        state.unread_mentions = read.unread_mentions;
                        state.last_viewed_at = read.timestamp;
                    }
                    deltas.push(StoreDelta::ThreadChanged(read.thread_id.clone()));
                }
            }
            WsEvent::ThreadFollowChanged(follow) => {
                if let Some(post) = self.posts.get_mut(&follow.thread_id) {
                    let post = Arc::make_mut(post);
                    post.is_following = Some(follow.state);
                    post.reply_count = follow.reply_count;
                    deltas.push(StoreDelta::PostsChanged(vec![follow.thread_id.clone()]));
                }
                if !follow.state
                    && let Some(state) = self.thread_states.remove(&follow.thread_id)
                {
                    self.adjust_thread_counts(
                        &broadcast.team_id,
                        (state.unread_replies, state.unread_mentions),
                        (0, 0),
                        &mut deltas,
                    );
                }
                deltas.push(StoreDelta::ThreadChanged(follow.thread_id.clone()));
            }
            WsEvent::SidebarCategoryCreated(_) | WsEvent::SidebarCategoryDeleted(_) => {
                if !broadcast.team_id.is_empty() {
                    deltas.push(StoreDelta::Fetch(Fetch::Categories(
                        broadcast.team_id.clone(),
                    )));
                }
            }
            WsEvent::SidebarCategoryUpdated(updated) => {
                // Every preference change also sends this without data or team: the webapp
                // ignores those (WebApp.md §7.6).
                if updated.is_empty() {
                    if !broadcast.team_id.is_empty() {
                        deltas.push(StoreDelta::Fetch(Fetch::Categories(
                            broadcast.team_id.clone(),
                        )));
                    }
                } else {
                    for category in updated {
                        let team = category.team_id.clone();
                        let list = self.categories.entry(team.clone()).or_default();
                        match list.iter_mut().find(|known| known.id == category.id) {
                            Some(known) => *known = category.clone(),
                            None => list.push(category.clone()),
                        }
                        if !deltas.contains(&StoreDelta::CategoriesChanged(team.clone())) {
                            deltas.push(StoreDelta::CategoriesChanged(team));
                        }
                    }
                }
            }
            WsEvent::SidebarCategoryOrderUpdated(order) => {
                if let Some(list) = self.categories.get_mut(&broadcast.team_id) {
                    list.sort_by_key(|category| {
                        order
                            .order
                            .iter()
                            .position(|id| *id == category.id)
                            .unwrap_or(usize::MAX)
                    });
                    deltas.push(StoreDelta::CategoriesChanged(broadcast.team_id.clone()));
                }
            }
            WsEvent::EmojiAdded(emoji) => {
                self.emoji.insert(emoji.name.clone(), emoji.clone());
                deltas.push(StoreDelta::EmojiChanged);
            }
            WsEvent::RoleUpdated(role) => {
                self.roles.insert(role.name.clone(), role.as_ref().clone());
                deltas.push(StoreDelta::RolesChanged);
            }
            WsEvent::ConfigChanged(config) => {
                let crt_before = self.crt_enabled();
                self.config = config.clone();
                deltas.push(StoreDelta::ConfigChanged);
                if self.crt_enabled() != crt_before {
                    deltas.push(StoreDelta::CrtChanged);
                }
            }
            WsEvent::LicenseChanged(license) => {
                self.license = license.clone();
                deltas.push(StoreDelta::LicenseChanged);
            }
            WsEvent::Hello(_) | WsEvent::Unknown(_) => {}
        }
        deltas
    }

    // ---- own actions ----

    /// Adds a post being sent. `post.pending_post_id` must be set (`"{user_id}:{now_ms}"`);
    /// it is also the post's id until the server's copy arrives.
    pub fn add_pending(&mut self, mut post: Post) -> Vec<StoreDelta> {
        let pending_id = post.pending_post_id.clone();
        post.id = PostId::new(pending_id.clone());
        post.user_id = self.me.clone();
        self.pending.insert(
            pending_id.clone(),
            PendingPost {
                channel: post.channel_id.clone(),
                root: post.root_id.clone(),
                failed: false,
            },
        );
        let mut deltas = vec![StoreDelta::PendingChanged(pending_id)];
        self.insert_post(&post, &mut deltas);
        deltas
    }

    /// The server's reply to `POST /posts`. The `posted` event carries the same post; whichever
    /// comes first replaces the pending post and the other is a plain update.
    pub fn confirm_post(&mut self, post: &Post) -> Vec<StoreDelta> {
        let mut deltas = Vec::new();
        if self.replace_pending(post, &mut deltas) {
            self.mark_read_local(&post.channel_id, post.create_at, false, &mut deltas);
        } else {
            self.insert_post(post, &mut deltas);
        }
        deltas
    }

    /// Sending failed: the post stays with Retry and Delete.
    pub fn fail_pending(&mut self, pending_post_id: &str) -> Vec<StoreDelta> {
        match self.pending.get_mut(pending_post_id) {
            Some(pending) => {
                pending.failed = true;
                vec![StoreDelta::PendingChanged(pending_post_id.to_owned())]
            }
            None => Vec::new(),
        }
    }

    /// Removes a failed (or dismissed) pending post.
    pub fn discard_pending(&mut self, pending_post_id: &str) -> Vec<StoreDelta> {
        let Some(pending) = self.pending.remove(pending_post_id) else {
            return Vec::new();
        };
        let id = PostId::new(pending_post_id);
        let mut deltas = vec![StoreDelta::PendingChanged(pending_post_id.to_owned())];
        self.unlink_post(&id, &pending.channel, &pending.root, &mut deltas);
        self.posts.remove(&id);
        deltas.push(StoreDelta::PostsRemoved(vec![id]));
        deltas
    }

    /// Marks a channel read locally after viewing it (the engine calls
    /// `POST /channels/members/me/view`).
    pub fn mark_viewed(&mut self, channel: &ChannelId, at: i64) -> Vec<StoreDelta> {
        self.manually_unread.remove(channel);
        let mut deltas = Vec::new();
        self.mark_read_local(channel, at, false, &mut deltas);
        deltas
    }

    // ---- helpers ----

    fn posted(&mut self, posted: &Posted, deltas: &mut Vec<StoreDelta>) {
        let post = &posted.post;
        if !self.replace_pending(post, deltas) {
            self.insert_post(post, deltas);
        }
        self.update_root_for_reply(post, &posted.followers, deltas);

        let channel_id = &post.channel_id;
        let crt = self.crt_enabled();
        let root = post.is_root();
        let counted = counts_as_message(post);
        let Some(channel) = self.channels.get_mut(channel_id) else {
            // A channel I don't know yet (a new DM, or the event raced the channel list).
            deltas.push(StoreDelta::Fetch(Fetch::Channel(channel_id.clone())));
            return;
        };
        {
            let channel = Arc::make_mut(channel);
            channel.last_post_at = channel.last_post_at.max(post.create_at);
            if root || !crt {
                channel.last_root_post_at = channel.last_root_post_at.max(post.create_at);
            }
            if counted {
                channel.total_msg_count += 1;
                if root {
                    channel.total_msg_count_root += 1;
                }
            }
        }
        deltas.push(StoreDelta::ChannelChanged(channel_id.clone()));

        let manually_unread = self.manually_unread.contains(channel_id);
        if !manually_unread && is_own_message(post, &self.me) {
            self.mark_read_local(channel_id, post.create_at, false, deltas);
            return;
        }
        if !manually_unread && self.viewing.as_ref() == Some(channel_id) {
            self.mark_read_local(channel_id, post.create_at, true, deltas);
            return;
        }
        if is_notification_suppressed(post) {
            return;
        }
        let Some(member) = self.channel_members.get_mut(channel_id) else {
            return;
        };
        if member.is_muted() && counted {
            // Muted: new messages don't make the channel unread (`onlyMentions`).
            member.msg_count += 1;
            if root {
                member.msg_count_root += 1;
            }
        }
        if posted.mentions.contains(&self.me) {
            member.mention_count += 1;
            if root {
                member.mention_count_root += 1;
            }
            if is_urgent(post) {
                member.urgent_mention_count += 1;
            }
        }
        deltas.push(StoreDelta::MemberChanged(channel_id.clone()));
    }

    fn post_edited(&mut self, post: &Post, deltas: &mut Vec<StoreDelta>) {
        if let Some(known) = self.posts.get(&post.id) {
            // Edits carry the whole post; keep reply data the event may lack.
            let mut post = post.clone();
            if post.participants.is_empty() {
                post.participants = known.participants.clone();
            }
            if post.is_following.is_none() {
                post.is_following = known.is_following;
            }
            self.posts.insert(post.id.clone(), Arc::new(post.clone()));
            deltas.push(StoreDelta::PostsChanged(vec![post.id]));
        } else {
            self.insert_post(post, deltas);
        }
    }

    fn post_deleted(&mut self, post: &Post, deltas: &mut Vec<StoreDelta>) {
        let mut removed = vec![post.id.clone()];
        self.unlink_post(&post.id, &post.channel_id, &post.root_id, deltas);
        self.posts.remove(&post.id);
        if post.is_root() {
            // The replies go with the root (the webapp removes them from the thread view),
            // whether or not the thread was opened.
            self.threads.remove(&post.id);
            let mut replies: Vec<(i64, PostId)> = self
                .posts
                .values()
                .filter(|reply| reply.root_id == post.id)
                .map(|reply| (reply.create_at, reply.id.clone()))
                .collect();
            replies.sort();
            for (_, reply) in replies {
                if let Some(timeline) = self.timelines.get_mut(&post.channel_id) {
                    timeline.remove(&reply);
                }
                self.posts.remove(&reply);
                removed.push(reply);
            }
            self.thread_states.remove(&post.id);
        } else if let Some(root) = self.posts.get_mut(&post.root_id) {
            let root = Arc::make_mut(root);
            root.reply_count = (root.reply_count - 1).max(0);
            deltas.push(StoreDelta::PostsChanged(vec![post.root_id.clone()]));
        }
        deltas.push(StoreDelta::PostsRemoved(removed));
    }

    pub(crate) fn reaction(
        &mut self,
        reaction: &Reaction,
        added: bool,
        deltas: &mut Vec<StoreDelta>,
    ) {
        let Some(post) = self.posts.get_mut(&reaction.post_id) else {
            return;
        };
        let post = Arc::make_mut(post);
        let reactions = &mut post.metadata.get_or_insert_with(Default::default).reactions;
        reactions.retain(|known| {
            !(known.user_id == reaction.user_id && known.emoji_name == reaction.emoji_name)
        });
        if added {
            reactions.push(reaction.clone());
        }
        post.has_reactions = !reactions.is_empty();
        deltas.push(StoreDelta::PostsChanged(vec![reaction.post_id.clone()]));
    }

    /// Stores a post and puts it into its channel timeline (roots, or all posts without CRT)
    /// and its thread.
    fn insert_post(&mut self, post: &Post, deltas: &mut Vec<StoreDelta>) {
        self.posts.insert(post.id.clone(), Arc::new(post.clone()));
        deltas.push(StoreDelta::PostsChanged(vec![post.id.clone()]));
        let key = (post.create_at, post.id.clone());
        if (post.is_root() || !self.crt_enabled())
            && let Some(timeline) = self.timelines.get_mut(&post.channel_id)
            && timeline.insert_live(key.clone())
        {
            deltas.push(StoreDelta::TimelineChanged(post.channel_id.clone()));
        }
        if !post.is_root()
            && let Some(thread) = self.threads.get_mut(&post.root_id)
            && thread.insert_live(key)
        {
            deltas.push(StoreDelta::ThreadChanged(post.root_id.clone()));
        }
    }

    /// Replaces my pending post with the server's copy. Returns whether one was pending.
    fn replace_pending(&mut self, post: &Post, deltas: &mut Vec<StoreDelta>) -> bool {
        if post.pending_post_id.is_empty() || post.user_id != self.me {
            return false;
        }
        let Some(pending) = self.pending.remove(&post.pending_post_id) else {
            return false;
        };
        let pending_id = PostId::new(post.pending_post_id.clone());
        self.unlink_post(&pending_id, &pending.channel, &pending.root, deltas);
        self.posts.remove(&pending_id);
        deltas.push(StoreDelta::PostsRemoved(vec![pending_id]));
        deltas.push(StoreDelta::PendingChanged(post.pending_post_id.clone()));
        self.insert_post(post, deltas);
        true
    }

    /// Takes a post out of its timeline and thread.
    fn unlink_post(
        &mut self,
        id: &PostId,
        channel: &ChannelId,
        root: &PostId,
        deltas: &mut Vec<StoreDelta>,
    ) {
        if let Some(timeline) = self.timelines.get_mut(channel)
            && timeline.remove(id)
        {
            deltas.push(StoreDelta::TimelineChanged(channel.clone()));
        }
        if !root.is_empty()
            && let Some(thread) = self.threads.get_mut(root)
            && thread.remove(id)
        {
            deltas.push(StoreDelta::ThreadChanged(root.clone()));
        }
    }

    /// A new reply bumps its root's reply count, last reply time and participants.
    fn update_root_for_reply(
        &mut self,
        post: &Post,
        followers: &[UserId],
        deltas: &mut Vec<StoreDelta>,
    ) {
        if post.is_root() {
            return;
        }
        let author = self.users.get(&post.user_id).cloned();
        let Some(root) = self.posts.get_mut(&post.root_id) else {
            return;
        };
        let root = Arc::make_mut(root);
        root.reply_count += 1;
        root.last_reply_at = root.last_reply_at.max(post.create_at);
        if let Some(author) = author
            && !root.participants.iter().any(|user| user.id == author.id)
        {
            root.participants.push(author.as_ref().clone());
        }
        if followers.contains(&self.me) {
            root.is_following = Some(true);
        }
        deltas.push(StoreDelta::PostsChanged(vec![post.root_id.clone()]));
    }

    /// Sets my counters to the channel's totals (`actionsToMarkChannelAsRead`).
    fn mark_read_local(
        &mut self,
        channel_id: &ChannelId,
        at: i64,
        notify_server: bool,
        deltas: &mut Vec<StoreDelta>,
    ) {
        let (Some(channel), Some(member)) = (
            self.channels.get(channel_id),
            self.channel_members.get_mut(channel_id),
        ) else {
            return;
        };
        member.msg_count = channel.total_msg_count;
        member.msg_count_root = channel.total_msg_count_root;
        member.mention_count = 0;
        member.mention_count_root = 0;
        member.urgent_mention_count = 0;
        member.last_viewed_at = member.last_viewed_at.max(at);
        deltas.push(StoreDelta::MemberChanged(channel_id.clone()));
        deltas.push(StoreDelta::MarkedRead {
            channel: channel_id.clone(),
            notify_server,
        });
    }

    fn adjust_thread_counts(
        &mut self,
        team: &TeamId,
        (old_replies, old_mentions): (i64, i64),
        (new_replies, new_mentions): (i64, i64),
        deltas: &mut Vec<StoreDelta>,
    ) {
        if team.is_empty() {
            return;
        }
        let counts = self.thread_counts.entry(team.clone()).or_default();
        counts.unread_threads += i64::from(new_replies > 0) - i64::from(old_replies > 0);
        counts.unread_threads = counts.unread_threads.max(0);
        counts.mentions = (counts.mentions + new_mentions - old_mentions).max(0);
        deltas.push(StoreDelta::ThreadCountsChanged(team.clone()));
    }

    fn remove_team(&mut self, team: &TeamId, deltas: &mut Vec<StoreDelta>) {
        if self.team_members.remove(team).is_none() && self.teams.remove(team).is_none() {
            return;
        }
        self.teams.remove(team);
        self.categories.remove(team);
        self.thread_counts.remove(team);
        let channels: Vec<ChannelId> = self
            .channels
            .values()
            .filter(|channel| channel.team_id == *team)
            .map(|channel| channel.id.clone())
            .collect();
        for channel in channels {
            self.channels.remove(&channel);
            self.channel_members.remove(&channel);
            self.timelines.remove(&channel);
            deltas.push(StoreDelta::ChannelRemoved(channel));
        }
        deltas.push(StoreDelta::TeamRemoved(team.clone()));
    }

    /// My preferences saved (from the server, or before it confirms); others' are ignored.
    pub fn put_preferences(&mut self, preferences: &[mm_model::Preference]) -> Vec<StoreDelta> {
        let crt_before = self.crt_enabled();
        let names_before = self.name_format();
        for preference in preferences.iter().filter(|p| p.user_id == self.me) {
            self.preferences.insert(
                (preference.category.clone(), preference.name.clone()),
                preference.value.clone(),
            );
        }
        let mut deltas = Vec::new();
        self.after_preferences(crt_before, names_before, &mut deltas);
        deltas
    }

    /// My preferences deleted.
    pub fn remove_preferences(&mut self, preferences: &[mm_model::Preference]) -> Vec<StoreDelta> {
        let crt_before = self.crt_enabled();
        let names_before = self.name_format();
        for preference in preferences.iter().filter(|p| p.user_id == self.me) {
            self.preferences
                .remove(&(preference.category.clone(), preference.name.clone()));
        }
        let mut deltas = Vec::new();
        self.after_preferences(crt_before, names_before, &mut deltas);
        deltas
    }

    /// Pins or unpins a post before the server's `post_edited` confirms it.
    pub fn set_post_pinned(&mut self, post: &PostId, pinned: bool) -> Vec<StoreDelta> {
        match self.posts.get_mut(post) {
            Some(known) if known.is_pinned != pinned => {
                Arc::make_mut(known).is_pinned = pinned;
                vec![StoreDelta::PostsChanged(vec![post.clone()])]
            }
            _ => Vec::new(),
        }
    }

    /// The server's copy of a post I edited (the patch response), unless a newer version
    /// (its `post_edited`, or a later edit) came first.
    pub fn put_edited_post(&mut self, post: &Post) -> Vec<StoreDelta> {
        if self
            .posts
            .get(&post.id)
            .is_some_and(|known| known.update_at > post.update_at)
        {
            return Vec::new();
        }
        let mut deltas = Vec::new();
        self.post_edited(post, &mut deltas);
        deltas
    }

    fn after_preferences(
        &self,
        crt_before: bool,
        names_before: crate::names::NameFormat,
        deltas: &mut Vec<StoreDelta>,
    ) {
        deltas.push(StoreDelta::PreferencesChanged);
        if self.crt_enabled() != crt_before {
            deltas.push(StoreDelta::CrtChanged);
        }
        if self.name_format() != names_before {
            deltas.push(StoreDelta::NamesChanged);
        }
    }
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
