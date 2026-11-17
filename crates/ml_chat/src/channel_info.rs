// SPDX-License-Identifier: AGPL-3.0-only

//! What the tool panel's Channel tab shows about the active chat, independent of the view
//! (MatterLite.md §10.6): favourite and muted state, the chat's link, whether it can be left,
//! and its members in the webapp's order.

use mm_model::{CategoryId, ChannelId, ChannelType, TeamId, UserId};
use mm_store::{Store, dm_partner};

use crate::chats::Presence;

/// The Favorites category of the chat's team and where the chat goes when it stops being a
/// favourite (Channels, or Direct Messages for DMs and GMs), as the webapp's
/// `favoriteChannel`/`unfavoriteChannel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub team: TeamId,
    pub is_favorite: bool,
    /// The category to move the chat into to toggle it.
    pub target: CategoryId,
}

/// `team` is the team shown in the Chats tab, used for DMs and GMs (they have none).
pub fn favorite(store: &Store, channel: &ChannelId, team: Option<&TeamId>) -> Option<Favorite> {
    let info = store.channels.get(channel)?;
    let team = if info.team_id.is_empty() {
        team?.clone()
    } else {
        info.team_id.clone()
    };
    let categories = store.categories.get(&team)?;
    let of_type = |kind: &str| categories.iter().find(|c| c.category_type == kind);
    let favorites = of_type("favorites")?;
    let is_favorite = favorites.channel_ids.contains(channel);
    let target = if is_favorite {
        let home = match info.channel_type {
            ChannelType::Direct | ChannelType::Group => "direct_messages",
            _ => "channels",
        };
        of_type(home)?.id.clone()
    } else {
        favorites.id.clone()
    };
    Some(Favorite {
        team,
        is_favorite,
        target,
    })
}

/// Muted: `mark_unread` is `mention` (the webapp's `isChannelMuted`).
pub fn is_muted(store: &Store, channel: &ChannelId) -> bool {
    store
        .channel_members
        .get(channel)
        .and_then(|member| member.notify_props.get("mark_unread"))
        .map(String::as_str)
        == Some("mention")
}

/// Open and private channels can be left, except the team's Town Square (the webapp's
/// `canLeaveChannel` without the experimental settings). DMs and GMs are closed instead.
pub fn can_leave(store: &Store, channel: &ChannelId) -> bool {
    store.channels.get(channel).is_some_and(|info| {
        matches!(info.channel_type, ChannelType::Open | ChannelType::Private)
            && info.name != "town-square"
            && info.delete_at == 0
    })
}

/// The chat's link in the webapp (`getChannelURL`): `{site}/{team}/channels/{name}`, DMs as
/// `{site}/{team}/messages/@{username}`, GMs as `{site}/{team}/messages/{name}`. `site` is the
/// server's `SiteURL`, else the address MatterLite uses.
pub fn link(
    store: &Store,
    channel: &ChannelId,
    team: Option<&TeamId>,
    server: &str,
) -> Option<String> {
    let info = store.channels.get(channel)?;
    let team_url = team_url(store, channel, team, server)?;
    let path = match info.channel_type {
        ChannelType::Direct => {
            let partner = dm_partner(&info.name, &store.me)?;
            format!("messages/@{}", store.users.get(&partner)?.username)
        }
        ChannelType::Group => format!("messages/{}", info.name),
        _ => format!("channels/{}", info.name),
    };
    Some(format!("{team_url}/{path}"))
}

/// A post's permalink in the webapp (`getPermalinkURL`): `{site}/{team}/pl/{post id}`, the
/// team of the post's channel, or `team` for a DM or GM.
pub fn post_link(
    store: &Store,
    post: &mm_model::PostId,
    team: Option<&TeamId>,
    server: &str,
) -> Option<String> {
    let channel = &store.posts.get(post)?.channel_id;
    Some(format!(
        "{}/pl/{post}",
        team_url(store, channel, team, server)?
    ))
}

/// `{site}/{team name}` for a chat; `team` stands in for a DM's or GM's.
fn team_url(
    store: &Store,
    channel: &ChannelId,
    team: Option<&TeamId>,
    server: &str,
) -> Option<String> {
    let info = store.channels.get(channel)?;
    let team_id = if info.team_id.is_empty() {
        team?
    } else {
        &info.team_id
    };
    let team_name = &store.teams.get(team_id)?.name;
    let site = store
        .config
        .get("SiteURL")
        .filter(|url| !url.is_empty())
        .unwrap_or(server)
        .trim_end_matches('/');
    Some(format!("{site}/{team_name}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRow {
    pub id: UserId,
    pub name: String,
    pub username: String,
    pub presence: Presence,
    pub me: bool,
}

/// The loaded members, online first, then away, do not disturb and offline, each by name.
pub fn members(store: &Store, channel: &ChannelId) -> Option<Vec<MemberRow>> {
    let ids = store.channel_users.get(channel)?;
    let format = store.name_format();
    let mut rows: Vec<MemberRow> = ids
        .iter()
        .filter_map(|id| store.users.get(id))
        .filter(|user| !user.is_deleted())
        .map(|user| MemberRow {
            id: user.id.clone(),
            name: format.display(user),
            username: user.username.clone(),
            presence: Presence::from_status(
                store
                    .statuses
                    .get(&user.id)
                    .map(|status| status.status.as_str()),
            ),
            me: user.id == store.me,
        })
        .collect();
    let rank = |presence: Presence| match presence {
        Presence::Online => 0,
        Presence::Away => 1,
        Presence::Dnd => 2,
        Presence::Offline => 3,
    };
    rows.sort_by(|a, b| {
        rank(a.presence)
            .cmp(&rank(b.presence))
            .then_with(|| mm_store::compare_names(&a.name, &b.name))
    });
    Some(rows)
}

#[cfg(test)]
#[path = "channel_info_tests.rs"]
mod tests;
