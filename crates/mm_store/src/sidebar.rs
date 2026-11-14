// SPDX-License-Identifier: AGPL-3.0-only

//! The channel sidebar of a team (WebApp.md §14): categories with their channels filtered and
//! sorted as the webapp does (`makeFilterAndSortChannelsForCategory` in
//! `MR/selectors/entities/channel_categories.ts`), and the optional Unreads group.

use std::cmp::Ordering;
use std::collections::HashSet;

use mm_model::preference::keys;
use mm_model::{Channel, ChannelId, ChannelType, Preference, SidebarCategory, TeamId};

use crate::names::dm_partner;
use crate::store::Store;
use crate::unread::ChannelUnread;

/// `sidebar_settings` preferences.
const SIDEBAR_SETTINGS: &str = "sidebar_settings";
const LIMIT_VISIBLE_DMS_GMS: &str = "limit_visible_dms_gms";
const SHOW_UNREAD_SECTION: &str = "show_unread_section";
/// `getVisibleDmGmLimit`'s default.
pub const DEFAULT_DM_GM_LIMIT: usize = 40;
const CHANNEL_APPROXIMATE_VIEW_TIME: &str = "channel_approximate_view_time";
const CHANNEL_OPEN_TIME: &str = "channel_open_time";

#[derive(Clone, Debug, PartialEq)]
pub struct Sidebar {
    /// Unread channels grouped at the top (`sidebar_settings/show_unread_section`); they are
    /// then left out of their categories. `None` when the group is off.
    pub unreads: Option<Vec<SidebarChannel>>,
    pub categories: Vec<SidebarCategoryView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarCategoryView {
    pub category: SidebarCategory,
    /// Filtered and sorted. A collapsed category shows only the channels with
    /// [`SidebarChannel::shown_when_collapsed`].
    pub channels: Vec<SidebarChannel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarChannel {
    pub id: ChannelId,
    pub name: String,
    pub channel_type: ChannelType,
    pub unread: ChannelUnread,
    pub archived: bool,
    pub current: bool,
}

impl SidebarChannel {
    pub fn shown_when_collapsed(&self) -> bool {
        self.current || self.unread.is_unread()
    }
}

impl Store {
    /// The sidebar of a team. `current` is the selected channel: it stays visible even when
    /// archived, closed or beyond the DM limit.
    pub fn sidebar(&self, team: &TeamId, current: Option<&ChannelId>) -> Sidebar {
        let show_unreads = self.show_unread_section();
        let mut unreads = Vec::new();
        let mut categories = Vec::new();
        for category in self.categories.get(team).into_iter().flatten() {
            let mut channels = self.category_channels(category, current);
            if show_unreads {
                channels.retain(|channel| {
                    if channel.unread.is_unread() {
                        unreads.push(channel.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            categories.push(SidebarCategoryView {
                category: category.clone(),
                channels,
            });
        }
        Sidebar {
            unreads: show_unreads.then(|| self.sort_unreads(unreads)),
            categories,
        }
    }

    /// Preferences the webapp saves so unread DMs and GMs stay visible after they're read
    /// (`loadProfilesForDM`/`loadProfilesForGM`): `direct_channel_show/<user>` and
    /// `group_channel_show/<channel>` = `true` for unread ones that don't have it. The engine
    /// sends these with `PUT /users/{id}/preferences` after the bootstrap and after new posts.
    pub fn dm_gm_show_preferences_to_save(&self) -> Vec<Preference> {
        let mut out = Vec::new();
        for channel in self.channels.values() {
            let Some(key) = self.show_preference_key(channel) else {
                continue;
            };
            if self.preference(key.0, &key.1) == Some("true") {
                continue;
            }
            if self
                .channel_unread(&channel.id)
                .is_some_and(|unread| unread.is_unread())
            {
                out.push(Preference {
                    user_id: self.me.clone(),
                    category: key.0.to_owned(),
                    name: key.1,
                    value: "true".to_owned(),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn show_unread_section(&self) -> bool {
        match self.preference(SIDEBAR_SETTINGS, SHOW_UNREAD_SECTION) {
            Some(value) => value == "true",
            None => self.config.get("ExperimentalGroupUnreadChannels") == Some("default_on"),
        }
    }

    /// `limit_visible_dms_gms`, default 40.
    pub fn dm_gm_limit(&self) -> usize {
        self.preference(SIDEBAR_SETTINGS, LIMIT_VISIBLE_DMS_GMS)
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_DM_GM_LIMIT)
    }

    fn category_channels(
        &self,
        category: &SidebarCategory,
        current: Option<&ChannelId>,
    ) -> Vec<SidebarChannel> {
        let crt = self.crt_enabled();
        let is_current = |channel: &Channel| current == Some(&channel.id);
        let unread = |channel: &Channel| {
            self.channel_unread(&channel.id)
                .is_some_and(|unread| unread.is_unread())
        };
        let mut seen = HashSet::new();
        let mut channels: Vec<&Channel> = category
            .channel_ids
            .iter()
            .filter(|id| seen.insert(*id))
            .filter_map(|id| self.channels.get(id).map(AsRef::as_ref))
            .filter(|channel| self.channel_members.contains_key(&channel.id))
            // Archived channels only while selected (`makeFilterArchivedChannels`).
            .filter(|channel| !channel.is_deleted() || is_current(channel))
            // Closed DMs/GMs (`makeFilterManuallyClosedDMs`): shown only with the show
            // preference, while unread, or while selected.
            .filter(|channel| match self.show_preference_key(channel) {
                None => true,
                Some(key) => {
                    unread(channel)
                        || is_current(channel)
                        || self
                            .preference(key.0, &key.1)
                            .is_some_and(|value| value != "false")
                }
            })
            .collect();

        if category.category_type == "direct_messages" {
            channels = self.filter_autoclosed(channels, current);
        }

        match category.sorting.as_str() {
            "manual" => {}
            "recent" => channels.sort_by_key(|channel| std::cmp::Reverse(recency(channel, crt))),
            _ => {
                let mut named: Vec<(String, &Channel)> = channels
                    .into_iter()
                    .map(|channel| (self.channel_name(&channel.id).unwrap_or_default(), channel))
                    .collect();
                named.sort_by(|(a_name, a), (b_name, b)| {
                    self.is_muted(a)
                        .cmp(&self.is_muted(b))
                        .then_with(|| compare_names(a_name, b_name))
                });
                channels = named.into_iter().map(|(_, channel)| channel).collect();
            }
        }

        channels
            .into_iter()
            .map(|channel| self.sidebar_channel(channel, current))
            .collect()
    }

    /// `makeFilterAutoclosedDMs`: in the DM category, DMs with deactivated users go (unless
    /// opened after the deactivation), and only the `limit_visible_dms_gms` most recently
    /// viewed stay, with unread ones and the selected one always kept.
    fn filter_autoclosed<'a>(
        &self,
        channels: Vec<&'a Channel>,
        current: Option<&ChannelId>,
    ) -> Vec<&'a Channel> {
        let unread = |channel: &Channel| {
            self.channel_unread(&channel.id)
                .is_some_and(|unread| unread.is_unread())
        };
        let mut unread_count = 0;
        let mut visible: Vec<&Channel> = channels
            .iter()
            .copied()
            .filter(|channel| {
                if unread(channel) {
                    unread_count += 1;
                    return true;
                }
                if current == Some(&channel.id) {
                    return true;
                }
                if channel.channel_type == ChannelType::Direct {
                    let teammate =
                        dm_partner(&channel.name, &self.me).and_then(|id| self.users.get(&id));
                    match teammate {
                        Some(teammate) if teammate.delete_at <= self.last_viewed(channel) => {}
                        _ => return false,
                    }
                }
                true
            })
            .collect();
        visible.sort_by(|a, b| {
            let current_first = (current != Some(&a.id)).cmp(&(current != Some(&b.id)));
            current_first
                .then_with(|| unread(b).cmp(&unread(a)))
                .then_with(|| self.last_viewed(b).cmp(&self.last_viewed(a)))
        });
        visible.truncate(self.dm_gm_limit().max(unread_count));
        let keep: HashSet<&ChannelId> = visible.iter().map(|channel| &channel.id).collect();
        channels
            .into_iter()
            .filter(|channel| keep.contains(&channel.id))
            .collect()
    }

    /// The last view of a channel: the member's `last_viewed_at` or the older view-time
    /// preferences, whichever is newest (the server only moves `last_viewed_at` to the last
    /// post's time).
    fn last_viewed(&self, channel: &Channel) -> i64 {
        let from_preference = |category: &str| {
            self.preference(category, channel.id.as_str())
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0)
        };
        self.channel_members
            .get(&channel.id)
            .map_or(0, |member| member.last_viewed_at)
            .max(from_preference(CHANNEL_APPROXIMATE_VIEW_TIME))
            .max(from_preference(CHANNEL_OPEN_TIME))
    }

    /// `sortUnreadChannels`: muted last, then channels with mentions, then most recent.
    fn sort_unreads(&self, mut channels: Vec<SidebarChannel>) -> Vec<SidebarChannel> {
        let crt = self.crt_enabled();
        channels.sort_by(|a, b| {
            a.unread
                .muted
                .cmp(&b.unread.muted)
                .then_with(|| (b.unread.mentions > 0).cmp(&(a.unread.mentions > 0)))
                .then_with(|| {
                    let at = |id: &ChannelId| {
                        self.channels
                            .get(id)
                            .map_or(0, |channel| recency(channel, crt))
                    };
                    at(&b.id).cmp(&at(&a.id))
                })
        });
        channels
    }

    fn sidebar_channel(&self, channel: &Channel, current: Option<&ChannelId>) -> SidebarChannel {
        SidebarChannel {
            id: channel.id.clone(),
            name: self.channel_name(&channel.id).unwrap_or_default(),
            channel_type: channel.channel_type.clone(),
            unread: self.channel_unread(&channel.id).unwrap_or_default(),
            archived: channel.is_deleted(),
            current: current == Some(&channel.id),
        }
    }

    fn is_muted(&self, channel: &Channel) -> bool {
        self.channel_members
            .get(&channel.id)
            .is_some_and(|member| member.is_muted())
    }

    /// The show preference of a DM (`direct_channel_show/<teammate>`) or GM
    /// (`group_channel_show/<channel>`).
    fn show_preference_key(&self, channel: &Channel) -> Option<(&'static str, String)> {
        match channel.channel_type {
            ChannelType::Direct => {
                // `loadProfilesForDM` strips my id from the name, which leaves "" for the
                // self-DM.
                let partner = dm_partner(&channel.name, &self.me)?;
                let name = if partner == self.me {
                    String::new()
                } else {
                    partner.to_string()
                };
                Some((keys::DIRECT_CHANNEL_SHOW, name))
            }
            ChannelType::Group => Some((keys::GROUP_CHANNEL_SHOW, channel.id.to_string())),
            _ => None,
        }
    }
}

/// `makeSortChannelsByRecency`: the last (root) post, or creation for an empty channel.
fn recency(channel: &Channel, crt: bool) -> i64 {
    let last = if crt && channel.last_root_post_at != 0 {
        channel.last_root_post_at
    } else {
        channel.last_post_at
    };
    last.max(channel.create_at)
}

/// Case-insensitive, with runs of digits compared as numbers (`localeCompare` with
/// `{numeric: true}`; a full locale collation can replace this later).
pub fn compare_names(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(digit);
                        chars.next();
                    }
                    digits
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let (x_trim, y_trim) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = x_trim
                    .len()
                    .cmp(&y_trim.len())
                    .then_with(|| x_trim.cmp(y_trim));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase()).then(Ordering::Equal);
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
#[path = "sidebar_tests.rs"]
mod tests;
