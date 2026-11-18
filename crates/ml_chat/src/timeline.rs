// SPDX-License-Identifier: AGPL-3.0-only

//! What a chat's timeline shows, independent of the view: the rows built from
//! [`Store::timeline_items`], combined join/leave messages worded as the webapp's
//! `CombinedSystemMessage`, authors, times and the rendering context for `mm_render`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{Datelike as _, NaiveDate, Timelike as _};
use mm_model::{ChannelId, Post, PostId, TeamId, User, UserId};
use mm_render::{ChannelInfo, Context, Lookup, UserInfo};
use mm_store::{ItemOptions, Store, TimelineItem, Zone};
use mm_sync::ImageKey;

/// One row of the timeline, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// The channel's first post is loaded: "Beginning of …".
    Start,
    Date(NaiveDate),
    NewMessages,
    Post {
        post: Arc<Post>,
        /// Same author shortly after the previous post: no header.
        consecutive: bool,
        pending: Option<Pending>,
    },
    /// Consecutive join/leave/add/remove messages; `text` is Markdown.
    Activity {
        first: PostId,
        create_at: i64,
        text: String,
    },
    Hole(HoleKind),
    /// In a thread, between the root post and the replies: "3 replies".
    ReplyCount(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    Sending,
    Failed,
}

/// What a hole in the loaded history is and how it gets filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoleKind {
    /// Older history above the newest stretch: `LoadOlder` (scrolling near it, or "Load").
    Older,
    /// Newer posts below a stretch opened at the first unread post: `LoadNewer`.
    Newer,
    /// Between two older stretches; the history backfill fills it.
    Gap,
    /// A thread's replies (or its root) not loaded yet; `OpenThread` loads them.
    Replies,
}

/// A row's identity across reloads: anchors the scroll position and keys the height cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowKey {
    Start,
    Date(NaiveDate),
    NewMessages,
    Post(PostId),
    Activity(PostId),
    /// By the post right below it (`None`: at the bottom).
    Hole(Option<PostId>),
    ReplyCount,
}

impl Row {
    pub fn key(&self, next: Option<&Row>) -> RowKey {
        match self {
            Self::Start => RowKey::Start,
            Self::Date(date) => RowKey::Date(*date),
            Self::NewMessages => RowKey::NewMessages,
            Self::Post { post, .. } => RowKey::Post(post.id.clone()),
            Self::Activity { first, .. } => RowKey::Activity(first.clone()),
            Self::Hole(_) => RowKey::Hole(next.and_then(Row::post_id).cloned()),
            Self::ReplyCount(_) => RowKey::ReplyCount,
        }
    }

    fn post_id(&self) -> Option<&PostId> {
        match self {
            Self::Post { post, .. } => Some(&post.id),
            Self::Activity { first, .. } => Some(first),
            _ => None,
        }
    }
}

/// Names listed in a reaction's tooltip before "and N other users".
const MAX_REACTION_NAMES: usize = 20;

/// One emoji's reactions to a post, as its chip shows them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReactionChip {
    pub name: String,
    pub count: usize,
    /// One of them is mine.
    pub mine: bool,
    /// Who reacted, and what a click does.
    pub tooltip: String,
}

/// A post's reactions grouped by emoji, in the order each emoji was first added.
/// `can_add`/`can_remove`: whether a click adds or removes mine (for the tooltip's hint).
pub fn reaction_chips(
    store: &Store,
    post: &Post,
    can_add: bool,
    can_remove: bool,
) -> Vec<ReactionChip> {
    let Some(metadata) = &post.metadata else {
        return Vec::new();
    };
    let mut sorted: Vec<_> = metadata.reactions.iter().collect();
    sorted.sort_by_key(|reaction| reaction.create_at);
    // Emoji name, whether one is mine, and the others' names.
    let mut groups: Vec<(String, bool, Vec<String>)> = Vec::new();
    for reaction in sorted {
        let index = match groups
            .iter()
            .position(|(name, _, _)| *name == reaction.emoji_name)
        {
            Some(index) => index,
            None => {
                groups.push((reaction.emoji_name.clone(), false, Vec::new()));
                groups.len() - 1
            }
        };
        let group = &mut groups[index];
        if reaction.user_id == store.me {
            group.1 = true;
        } else {
            group.2.push(
                store
                    .user_name(&reaction.user_id)
                    .unwrap_or_else(|| "Someone".to_owned()),
            );
        }
    }
    groups
        .into_iter()
        .map(|(name, mine, others)| {
            let hint = match mine {
                true if can_remove => Some("(click to remove)"),
                false if can_add => Some("(click to add)"),
                _ => None,
            };
            ReactionChip {
                tooltip: reaction_tooltip(mine, &others, &name, hint),
                count: others.len() + usize::from(mine),
                name,
                mine,
            }
        })
        .collect()
}

/// "You, bob and carol reacted with :smile: (click to remove)" (`reaction.reacted`,
/// `reaction.usersReacted`, `reaction.usersAndOthersReacted`).
pub fn reaction_tooltip(mine: bool, others: &[String], emoji: &str, hint: Option<&str>) -> String {
    let mut users: Vec<&str> = Vec::new();
    if mine {
        users.push("You");
    }
    users.extend(others.iter().map(String::as_str));
    let names = if users.len() > MAX_REACTION_NAMES {
        let rest = users.len() - MAX_REACTION_NAMES;
        let unit = if rest == 1 { "user" } else { "users" };
        format!(
            "{} and {rest} other {unit}",
            users[..MAX_REACTION_NAMES].join(", ")
        )
    } else if let [first @ .., last] = users.as_slice()
        && !first.is_empty()
    {
        format!("{} and {last}", first.join(", "))
    } else {
        users.first().copied().unwrap_or_default().to_owned()
    };
    let text = format!("{names} reacted with :{emoji}:");
    match hint {
        Some(hint) => format!("{text} {hint}"),
        None => text,
    }
}

/// `isPostInteractable`: not pending or failed, deleted, or a system or ephemeral message.
pub fn interactable(post: &Post, pending: bool) -> bool {
    !pending && !post.is_deleted() && !post.is_system()
}

/// The post `↑` in an empty composer edits (`getLatestPostToEdit`): my newest interactable
/// post among the shown ones, given oldest first with whether each is pending. Whether I may
/// still edit it is for [`Store::post_actions`] to say.
pub fn latest_to_edit<'a>(
    posts: impl DoubleEndedIterator<Item = (&'a Post, bool)>,
    me: &UserId,
) -> Option<PostId> {
    posts
        .rev()
        .find(|(post, pending)| post.user_id == *me && interactable(post, *pending))
        .map(|(post, _)| post.id.clone())
}

/// The rows of a thread, oldest first (the webapp's RHS thread viewer): the root post, the
/// reply count, then the replies with date separators, a reply right after another by the
/// same author shown without its header. Without the thread loaded, a [`HoleKind::Replies`]
/// row stands for what is missing.
pub fn thread_rows(store: &Store, root: &PostId) -> Vec<Row> {
    let Some(root_post) = store.posts.get(root) else {
        return vec![Row::Hole(HoleKind::Replies)];
    };
    let zone = store
        .users
        .get(&store.me)
        .map_or(Zone::Local, |me| Zone::of(me));
    let pending = |post: &Post| {
        store.pending.get(post.id.as_str()).map(|state| {
            if state.failed {
                Pending::Failed
            } else {
                Pending::Sending
            }
        })
    };
    let mut date = zone.date(root_post.create_at);
    let mut rows = vec![
        Row::Date(date),
        Row::Post {
            post: root_post.clone(),
            consecutive: false,
            pending: None,
        },
    ];
    let Some(thread) = store.threads.get(root) else {
        rows.push(Row::Hole(HoleKind::Replies));
        return rows;
    };
    let replies: Vec<&Arc<Post>> = thread
        .post_ids()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|id| store.posts.get(id))
        .filter(|post| !post.is_deleted())
        .collect();
    if !replies.is_empty() {
        rows.push(Row::ReplyCount(replies.len()));
    }
    let mut previous: Option<&Arc<Post>> = None;
    for reply in replies {
        let reply_date = zone.date(reply.create_at);
        if reply_date != date {
            rows.push(Row::Date(reply_date));
            date = reply_date;
            previous = None;
        }
        rows.push(Row::Post {
            post: reply.clone(),
            consecutive: previous.is_some_and(|previous| mm_store::consecutive(reply, previous)),
            pending: pending(reply),
        });
        previous = Some(reply);
    }
    rows
}

/// "1 reply", "3 replies".
pub fn replies_label(count: usize) -> String {
    if count == 1 {
        "1 reply".to_owned()
    } else {
        format!("{count} replies")
    }
}

/// The rows of a channel, oldest first. `last_viewed_at` places the "New messages" line.
pub fn rows(store: &Store, channel: &ChannelId, last_viewed_at: Option<i64>) -> Vec<Row> {
    let items = store.timeline_items(channel, &ItemOptions { last_viewed_at });
    let Some(timeline) = store.timelines.get(channel) else {
        return Vec::new();
    };
    let my_username = store
        .users
        .get(&store.me)
        .map(|me| me.username.as_str())
        .unwrap_or_default();
    let mut rows: Vec<Row> = Vec::with_capacity(items.len() + 1);
    let reaches_start =
        timeline.is_known_empty() || timeline.ranges().first().is_some_and(|range| range.start);
    if reaches_start {
        rows.push(Row::Start);
    }
    // Oldest first.
    for item in items.into_iter().rev() {
        rows.push(match item {
            TimelineItem::Date(date) => Row::Date(date),
            TimelineItem::NewMessages { .. } => Row::NewMessages,
            TimelineItem::Post { post, consecutive } => {
                let pending = store.pending.get(post.id.as_str()).map(|state| {
                    if state.failed {
                        Pending::Failed
                    } else {
                        Pending::Sending
                    }
                });
                Row::Post {
                    post,
                    consecutive,
                    pending,
                }
            }
            TimelineItem::UserActivity(posts) => Row::Activity {
                first: posts[0].id.clone(),
                create_at: posts[0].create_at,
                text: activity_markdown(&posts, my_username),
            },
            TimelineItem::Hole => Row::Hole(HoleKind::Gap),
        });
    }
    // Which holes can be loaded on demand.
    let holes: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, Row::Hole(_)))
        .map(|(index, _)| index)
        .collect();
    if timeline.has_recent() {
        if let Some(&last) = holes.last() {
            rows[last] = Row::Hole(HoleKind::Older);
        }
    } else if let Some(&last) = holes.last() {
        rows[last] = Row::Hole(HoleKind::Newer);
        if let Some(&before) = holes.iter().rev().nth(1) {
            // Opened at the first unread post: the stretch's older side loads too.
            if holes.len() == 2 {
                rows[before] = Row::Hole(HoleKind::Older);
            }
        }
    }
    rows
}

#[derive(Debug)]
struct Activity {
    post_type: String,
    actors: Vec<String>,
    targets: Vec<String>,
}

/// Combined membership messages in the webapp's words (`combineUserActivitySystemPost` and
/// `CombinedSystemMessage`): runs of one kind become one sentence, a join directly followed by
/// the same user's leave becomes "joined and left".
pub fn activity_markdown(posts: &[Arc<Post>], my_username: &str) -> String {
    let mut entries: Vec<Activity> = Vec::new();
    for post in posts {
        let prop = |key: &str| {
            post.props
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let actor = prop("username");
        let target = match post.post_type.as_str() {
            "system_add_to_channel" | "system_add_to_team" => prop("addedUsername"),
            "system_remove_from_channel" | "system_remove_from_team" => prop("removedUsername"),
            _ => String::new(),
        };
        let users_related = !target.is_empty() || post.post_type.contains("remove_from");
        let removed = post.post_type == "system_remove_from_channel";
        match entries.last_mut() {
            Some(previous)
                if previous.post_type == post.post_type
                    && users_related
                    && (previous.actors.first() == Some(&actor) || removed) =>
            {
                push_unique(&mut previous.targets, target);
            }
            Some(previous) if previous.post_type == post.post_type && !users_related => {
                push_unique(&mut previous.actors, actor);
            }
            Some(previous)
                if previous.post_type == "system_join_channel"
                    && post.post_type == "system_leave_channel"
                    && previous.actors == [actor.clone()] =>
            {
                previous.post_type = "join_leave_channel".to_owned();
            }
            _ => entries.push(Activity {
                post_type: post.post_type.clone(),
                actors: vec![actor],
                targets: if users_related {
                    vec![target]
                } else {
                    Vec::new()
                },
            }),
        }
    }
    entries
        .iter()
        .map(|entry| sentence(entry, my_username))
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_unique(list: &mut Vec<String>, name: String) {
    if !list.contains(&name) {
        list.push(name);
    }
}

fn sentence(entry: &Activity, my_username: &str) -> String {
    let (users, verb) = match entry.post_type.as_str() {
        "system_join_channel" => (&entry.actors, "**joined the channel**"),
        "system_leave_channel" => (&entry.actors, "**left the channel**"),
        "join_leave_channel" => (&entry.actors, "**joined and left the channel**"),
        "system_join_team" => (&entry.actors, "**joined the team**"),
        "system_leave_team" => (&entry.actors, "**left the team**"),
        "system_add_to_channel" => (&entry.targets, "**added to the channel**"),
        "system_add_to_team" => (&entry.targets, "**added to the team**"),
        "system_remove_from_channel" => (&entry.targets, "**removed from the channel**"),
        "system_remove_from_team" => (&entry.targets, "**removed from the team**"),
        _ => (&entry.actors, ""),
    };
    let you = users.len() == 1 && !my_username.is_empty() && users[0] == my_username;
    let names: Vec<String> = users.iter().map(|name| format!("@{name}")).collect();
    let subject = if you {
        "You".to_owned()
    } else {
        match names.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [first, second] => format!("{first} and {second}"),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    };
    let added = entry.post_type.contains("add_to");
    let removed = entry.post_type.contains("remove_from");
    // As the webapp's strings: "@a added …", "@a and @b added …", "@a, @b and @c were
    // added …"; "@a was removed …", "@a and @b were removed …"; "You were …".
    let be = match (added || removed, you, names.len()) {
        (false, _, _) => "",
        (true, true, _) => "were ",
        (true, false, 1) if removed => "was ",
        (true, false, 2) if removed => "were ",
        (true, false, 1 | 2) => "",
        (true, false, _) => "were ",
    };
    let by = if added {
        entry
            .actors
            .first()
            .filter(|actor| !actor.is_empty())
            .map(|actor| format!(" by @{actor}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!("{subject} {be}{verb}{by}.")
}

/// Who a post shows as its author.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    /// A bot or a webhook post: the BOT tag.
    pub bot: bool,
    /// The profile picture to show (none for webhook posts with their own name).
    pub picture: Option<ImageKey>,
    pub initials: String,
}

pub fn author(store: &Store, post: &Post) -> Author {
    let user = store.users.get(&post.user_id);
    let from_webhook = post
        .props
        .get("from_webhook")
        .and_then(|value| value.as_str())
        == Some("true");
    let override_name = post
        .props
        .get("override_username")
        .and_then(|value| value.as_str())
        .filter(|name| {
            from_webhook
                && !name.is_empty()
                && store.config.get("EnablePostUsernameOverride") == Some("true")
        });
    let name = match (override_name, user) {
        (Some(name), _) => name.to_owned(),
        (None, Some(user)) => store.name_format().display(user),
        (None, None) => "Someone".to_owned(),
    };
    Author {
        initials: initials(user.map(Arc::as_ref), &name),
        bot: from_webhook || user.is_some_and(|user| user.is_bot),
        picture: user
            .filter(|_| override_name.is_none())
            .map(|user| ImageKey::Avatar {
                user: user.id.clone(),
                updated: user.last_picture_update,
            }),
        name,
    }
}

fn initials(user: Option<&User>, name: &str) -> String {
    let from_names: String = user
        .map(|user| {
            [&user.first_name, &user.last_name]
                .iter()
                .filter_map(|part| part.chars().next())
                .collect()
        })
        .unwrap_or_default();
    let initials = if from_names.is_empty() {
        name.chars()
            .find(|c| c.is_alphanumeric())
            .map(String::from)
            .unwrap_or_default()
    } else {
        from_names
    };
    initials.to_uppercase()
}

/// The zone times are shown in and whether to use a 24-hour clock
/// (`display_settings/use_military_time`).
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub zone: Zone,
    pub military: bool,
}

impl Clock {
    pub fn of(store: &Store) -> Self {
        Self {
            zone: store
                .users
                .get(&store.me)
                .map_or(Zone::Local, |me| Zone::of(me)),
            military: store.preference("display_settings", "use_military_time") == Some("true"),
        }
    }

    /// `10:02 AM` or `10:02`.
    pub fn time(&self, at_ms: i64) -> String {
        let time = self.zone.local(at_ms).time();
        if self.military {
            format!("{:02}:{:02}", time.hour(), time.minute())
        } else {
            let (pm, hour) = time.hour12();
            format!(
                "{hour}:{:02} {}",
                time.minute(),
                if pm { "PM" } else { "AM" }
            )
        }
    }

    pub fn today(&self, now_ms: i64) -> NaiveDate {
        self.zone.date(now_ms)
    }
}

/// When a post was edited, as the webapp's edited indicator says it: "Edited today at
/// 10:02 AM", "Edited Monday, January 5 at 9:15 PM".
pub fn edited_label(clock: &Clock, edit_at: i64, now_ms: i64) -> String {
    let date = date_label(clock.zone.date(edit_at), clock.today(now_ms));
    let date = match date.as_str() {
        "Today" | "Yesterday" => date.to_lowercase(),
        _ => date,
    };
    format!("Edited {date} at {}", clock.time(edit_at))
}

/// A date separator: Today, Yesterday, the weekday and date this year, else with the year.
pub fn date_label(date: NaiveDate, today: NaiveDate) -> String {
    if date == today {
        return "Today".to_owned();
    }
    if Some(date) == today.pred_opt() {
        return "Yesterday".to_owned();
    }
    if date.year() == today.year() {
        date.format("%A, %B %-d").to_string()
    } else {
        date.format("%B %-d, %Y").to_string()
    }
}

/// What rendering a chat's messages needs from the store, copied out of it (so messages can
/// be rendered later, as they scroll into view, without holding the store): users by username
/// in the name display format, the team's channels by name, the custom emoji, my mention keys
/// (`getMentionKeysForPost`), whether channel-wide mentions notify me, custom URL schemes.
#[derive(Debug, Default)]
pub struct ChatLookup {
    users: HashMap<String, UserInfo>,
    channels: HashMap<String, ChannelInfo>,
    emoji: HashSet<String>,
    my_username: String,
    mention_keys: Vec<String>,
    channel_mentions_notify: bool,
    custom_url_schemes: Vec<String>,
}

impl ChatLookup {
    /// For posts in `channel`; `~name` links resolve in `team`.
    pub fn new(store: &Store, channel: &ChannelId, team: Option<&TeamId>) -> Self {
        let format = store.name_format();
        let mut lookup = Self {
            users: store
                .users
                .values()
                .map(|user| {
                    (
                        user.username.clone(),
                        UserInfo {
                            id: user.id.clone(),
                            display_name: format.display(user),
                        },
                    )
                })
                .collect(),
            channels: store
                .channels
                .values()
                .filter(|channel| team.is_some_and(|team| channel.team_id == *team))
                .map(|channel| {
                    (
                        channel.name.clone(),
                        ChannelInfo {
                            id: channel.id.clone(),
                            display_name: channel.display_name.clone(),
                        },
                    )
                })
                .collect(),
            emoji: store.emoji.keys().cloned().collect(),
            channel_mentions_notify: true,
            ..Self::default()
        };
        if let Some(me) = store.users.get(&store.me) {
            lookup.my_username = me.username.clone();
            let props = &me.notify_props;
            if props.get("first_name").map(String::as_str) == Some("true")
                && !me.first_name.is_empty()
            {
                lookup.mention_keys.push(me.first_name.to_lowercase());
            }
            if let Some(keys) = props.get("mention_keys") {
                lookup.mention_keys.extend(
                    keys.split(',')
                        .map(|key| key.trim().trim_start_matches('@').to_lowercase())
                        .filter(|key| !key.is_empty()),
                );
            }
            let channel_wide = props.get("channel").map(String::as_str) != Some("false");
            let ignored = store
                .channel_members
                .get(channel)
                .and_then(|member| member.notify_props.get("ignore_channel_mentions"))
                .map(String::as_str)
                == Some("on");
            lookup.channel_mentions_notify = channel_wide && !ignored;
        }
        if let Some(schemes) = store.config.get("CustomUrlSchemes") {
            lookup.custom_url_schemes = schemes
                .split(',')
                .map(|scheme| scheme.trim().to_owned())
                .filter(|scheme| !scheme.is_empty())
                .collect();
        }
        lookup
    }

    pub fn context(&self) -> Context<'_> {
        let mut context = Context::new(self, &self.my_username);
        context.mention_keys = self.mention_keys.clone();
        context.channel_mentions_notify = self.channel_mentions_notify;
        context.custom_url_schemes = self.custom_url_schemes.clone();
        context
    }

    /// A `~name` of the team.
    pub fn channel_id(&self, name: &str) -> Option<&ChannelId> {
        self.channels.get(name).map(|channel| &channel.id)
    }
}

impl Lookup for ChatLookup {
    fn user(&self, username: &str) -> Option<UserInfo> {
        self.users.get(username).cloned()
    }

    fn channel(&self, name: &str) -> Option<ChannelInfo> {
        self.channels.get(name).cloned()
    }

    fn custom_emoji(&self, name: &str) -> bool {
        self.emoji.contains(name)
    }
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;
