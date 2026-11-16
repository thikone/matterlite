// SPDX-License-Identifier: AGPL-3.0-only

//! What the Chats tab shows (MatterLite.md §10.6, WebApp.md §14), independent of the view: the
//! teams in the webapp's order with their badges, and the rows of one team's sidebar
//! (categories, collapsed ones showing only unread and current chats, or a flat list of the
//! chats matching a filter), plus moving through them with Alt+↑/↓.

use mm_model::{CategoryId, ChannelId, ChannelType, TeamId};
use mm_store::{ChannelUnread, SidebarChannel, Store, TeamUnread, compare_names, dm_partner};

/// `teams_order` preference: team ids separated by commas (the team sidebar's order).
const TEAMS_ORDER: &str = "teams_order";

#[derive(Clone, Debug, PartialEq)]
pub struct TeamEntry {
    pub id: TeamId,
    pub name: String,
    pub unread: TeamUnread,
}

/// My teams in the webapp's order: the `teams_order` preference, then the others by name.
pub fn teams(store: &Store) -> Vec<TeamEntry> {
    let order: Vec<&str> = store
        .preference(TEAMS_ORDER, "")
        .map(|value| value.split(',').map(str::trim).collect())
        .unwrap_or_default();
    let mut teams: Vec<_> = store
        .teams
        .values()
        .filter(|team| team.delete_at == 0)
        .map(|team| TeamEntry {
            id: team.id.clone(),
            name: team.display_name.clone(),
            unread: store.team_unread(&team.id),
        })
        .collect();
    teams.sort_by(|a, b| {
        let position = |team: &TeamEntry| order.iter().position(|id| *id == team.id.as_str());
        match (position(a), position(b)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => compare_names(&a.name, &b.name),
        }
    });
    teams
}

/// The badge of the team switcher: unread or mentions in the teams that aren't shown.
pub fn other_teams_unread(teams: &[TeamEntry], shown: Option<&TeamId>) -> TeamUnread {
    let mut total = TeamUnread::default();
    for team in teams.iter().filter(|team| Some(&team.id) != shown) {
        total.messages += team.unread.messages + team.unread.threads;
        total.mentions += team.unread.total_mentions();
    }
    total
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Online,
    Away,
    Dnd,
    Offline,
}

impl Presence {
    pub(crate) fn from_status(status: Option<&str>) -> Self {
        match status {
            Some("online") => Self::Online,
            Some("away") => Self::Away,
            Some("dnd") => Self::Dnd,
            _ => Self::Offline,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelIcon {
    Public,
    Private,
    Archived,
    /// A DM, with the other person's status.
    Direct(Presence),
    /// A GM with this many members besides me.
    Group(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelRow {
    pub id: ChannelId,
    pub name: String,
    pub icon: ChannelIcon,
    pub unread: ChannelUnread,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A category; `None` is the Unreads group, which can't be collapsed.
    Header {
        category: Option<CategoryId>,
        title: String,
        collapsed: bool,
    },
    Channel(ChannelRow),
    /// The filter matched nothing.
    Empty(String),
}

impl Row {
    pub fn channel(&self) -> Option<&ChannelRow> {
        match self {
            Row::Channel(row) => Some(row),
            _ => None,
        }
    }
}

/// The rows of `team`'s sidebar. With a `filter`, every chat of the team (DMs and GMs
/// included) whose name contains it, in sidebar order and without headers.
pub fn rows(store: &Store, team: &TeamId, current: Option<&ChannelId>, filter: &str) -> Vec<Row> {
    let sidebar = store.sidebar(team, current);
    let filter = filter.trim().to_lowercase();
    let row = |channel: &SidebarChannel| Row::Channel(channel_row(store, channel));
    if !filter.is_empty() {
        let mut rows: Vec<Row> = sidebar
            .unreads
            .iter()
            .flatten()
            .chain(sidebar.categories.iter().flat_map(|view| &view.channels))
            .filter(|channel| matches_filter(store, channel, &filter))
            .map(row)
            .collect();
        if rows.is_empty() {
            rows.push(Row::Empty(format!("No chats match \"{}\".", filter)));
        }
        return rows;
    }
    let mut rows = Vec::new();
    if let Some(unreads) = &sidebar.unreads
        && !unreads.is_empty()
    {
        rows.push(Row::Header {
            category: None,
            title: "Unreads".to_owned(),
            collapsed: false,
        });
        rows.extend(unreads.iter().map(row));
    }
    for view in &sidebar.categories {
        // As in the webapp, Favorites shows only when something is in it.
        if view.category.category_type == "favorites" && view.channels.is_empty() {
            continue;
        }
        rows.push(Row::Header {
            category: Some(view.category.id.clone()),
            title: view.category.display_name.clone(),
            collapsed: view.category.collapsed,
        });
        rows.extend(
            view.channels
                .iter()
                .filter(|channel| !view.category.collapsed || channel.shown_when_collapsed())
                .map(row),
        );
    }
    rows
}

fn matches_filter(store: &Store, channel: &SidebarChannel, filter: &str) -> bool {
    channel.name.to_lowercase().contains(filter)
        || store.channels.get(&channel.id).is_some_and(|full| {
            full.channel_type != ChannelType::Direct
                && full.channel_type != ChannelType::Group
                && full.name.to_lowercase().contains(filter)
        })
}

fn channel_row(store: &Store, channel: &SidebarChannel) -> ChannelRow {
    ChannelRow {
        id: channel.id.clone(),
        name: channel.name.clone(),
        icon: channel_icon(store, &channel.id),
        unread: channel.unread,
        current: channel.current,
    }
}

/// The icon of a chat: its type, the DM partner's status, the GM's other members.
pub fn channel_icon(store: &Store, channel: &ChannelId) -> ChannelIcon {
    let Some(full) = store.channels.get(channel) else {
        return ChannelIcon::Public;
    };
    match full.channel_type {
        _ if full.is_deleted() => ChannelIcon::Archived,
        ChannelType::Open => ChannelIcon::Public,
        ChannelType::Private => ChannelIcon::Private,
        ChannelType::Direct => {
            let status = dm_partner(&full.name, &store.me)
                .and_then(|partner| store.statuses.get(&partner))
                .map(|status| status.status.as_str());
            ChannelIcon::Direct(Presence::from_status(status))
        }
        ChannelType::Group => {
            ChannelIcon::Group(full.display_name.split(',').count().saturating_sub(1))
        }
        _ => ChannelIcon::Public,
    }
}

/// One result of the chat switcher (Ctrl/Cmd+K).
#[derive(Clone, Debug, PartialEq)]
pub struct SwitcherItem {
    pub id: ChannelId,
    pub name: String,
    /// The team's name for channels when I'm in several teams.
    pub team: Option<String>,
    pub icon: ChannelIcon,
    pub unread: ChannelUnread,
}

/// How many results the switcher lists.
pub const SWITCHER_RESULTS: usize = 12;

/// The chats for the switcher: every chat I'm in (not archived), across teams. Without a
/// query, the most recently viewed first; with one, fuzzy matches on the name (and a DM
/// partner's username) by score, then by last view.
pub fn switcher_items(store: &Store, query: &str) -> Vec<SwitcherItem> {
    let query = query.trim();
    let several_teams = store.teams.len() > 1;
    let mut scored: Vec<(i64, i64, SwitcherItem)> = store
        .channels
        .values()
        .filter(|channel| !channel.is_deleted())
        .filter_map(|channel| {
            let member = store.channel_members.get(&channel.id)?;
            let name = store.channel_name(&channel.id)?;
            let score = if query.is_empty() {
                0
            } else {
                let mut best = fuzzy_match::match_indices_case_insensitive(&name, query)
                    .map(|result| result.score);
                if channel.channel_type == ChannelType::Direct
                    && let Some(user) = dm_partner(&channel.name, &store.me)
                        .and_then(|partner| store.users.get(&partner))
                {
                    let by_username =
                        fuzzy_match::match_indices_case_insensitive(&user.username, query)
                            .map(|result| result.score);
                    best = best.max(by_username);
                }
                if !channel.channel_type.is_direct_or_group() {
                    let by_url = fuzzy_match::match_indices_case_insensitive(&channel.name, query)
                        .map(|result| result.score);
                    best = best.max(by_url);
                }
                best?
            };
            let team = (several_teams && !channel.team_id.is_empty())
                .then(|| store.teams.get(&channel.team_id))
                .flatten()
                .map(|team| team.display_name.clone());
            Some((
                score,
                member.last_viewed_at,
                SwitcherItem {
                    id: channel.id.clone(),
                    name,
                    team,
                    icon: channel_icon(store, &channel.id),
                    unread: store.channel_unread(&channel.id).unwrap_or_default(),
                },
            ))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then_with(|| compare_names(&a.2.name, &b.2.name))
    });
    scored
        .into_iter()
        .take(SWITCHER_RESULTS)
        .map(|(_, _, item)| item)
        .collect()
}

/// The chat `delta` rows away from `current` (Alt+↑/↓), or the next unread one
/// (Alt+Shift+↑/↓). Wraps around; without a current chat, starts at the top or bottom.
pub fn step(
    rows: &[Row],
    current: Option<&ChannelId>,
    delta: isize,
    unread_only: bool,
) -> Option<ChannelId> {
    let channels: Vec<&ChannelRow> = rows.iter().filter_map(Row::channel).collect();
    if channels.is_empty() || delta == 0 {
        return None;
    }
    let len = channels.len() as isize;
    let start = current.and_then(|id| channels.iter().position(|row| &row.id == id));
    let mut index = match start {
        Some(index) => index as isize,
        None if delta > 0 => -1,
        None => len,
    };
    for _ in 0..len {
        index = (index + delta.signum()).rem_euclid(len);
        let row = channels[index as usize];
        if Some(index as usize) == start {
            return None;
        }
        if !unread_only || row.unread.is_unread() {
            return Some(row.id.clone());
        }
    }
    None
}

#[cfg(test)]
#[path = "chats_tests.rs"]
mod tests;
