// SPDX-License-Identifier: AGPL-3.0-only

//! The P0 bootstrap (WebApp.md §6, MatterLite.md §8.5) and the `since` catch-up of loaded
//! timelines (§8.7).

use std::collections::BTreeSet;

use mm_api::{ApiError, Client, PostsQuery};
use mm_model::{ChannelId, ChannelType, UserId};
use mm_store::{PageRequest, StoreDelta, dm_partner};

use crate::SharedStore;

/// The server returns at most this many posts per `since` request (`GetPostsSince` in
/// `post_store.go`); a full reply means there may be more, so the posts can't be assumed
/// contiguous.
pub const SINCE_CAP: usize = 1000;
/// `since` goes back this far before the last known sync time, for clock differences.
pub const SINCE_MARGIN: i64 = 60_000;
/// Page size of channel pages (`POST_CHUNK_SIZE`).
pub(crate) const PAGE: u32 = 60;

/// Fetches everything P0 needs and loads it into the store, keeping the cached posts and
/// timelines of channels that are still mine.
pub(crate) async fn bootstrap(client: &Client, store: &SharedStore) -> Result<(), ApiError> {
    let (config, license) = tokio::try_join!(client.client_config(), client.client_license())?;
    let (me, preferences, teams, team_members) = tokio::try_join!(
        client.me(),
        client.my_preferences(),
        client.my_teams(),
        client.my_team_members(),
    )?;
    let crt_preference = preferences
        .iter()
        .find(|p| p.category == "display_settings" && p.name == "collapsed_reply_threads")
        .map(|p| p.value.clone());
    let crt = config
        .collapsed_threads()
        .is_enabled(crt_preference.as_deref());
    let (channels, members, unreads) = tokio::try_join!(
        client.my_channels(),
        client.my_channel_members(&me.id),
        client.my_team_unreads(crt),
    )?;
    let mut categories = Vec::new();
    for team in &teams {
        categories.push((
            team.id.clone(),
            client.sidebar_categories(&me.id, &team.id).await?,
        ));
    }
    let mut role_names: BTreeSet<&str> = me.role_names().collect();
    role_names.extend(team_members.iter().flat_map(|m| m.roles.split_whitespace()));
    role_names.extend(members.iter().flat_map(|m| m.roles.split_whitespace()));
    let role_names: Vec<&str> = role_names.into_iter().collect();
    let roles = client.roles_by_names(&role_names).await?;

    // Profiles and statuses for the sidebar: DM partners and GM members by username.
    let partners: Vec<UserId> = channels
        .iter()
        .filter(|channel| channel.channel_type == ChannelType::Direct)
        .filter_map(|channel| dm_partner(&channel.name, &me.id))
        .filter(|id| *id != me.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let (profiles, statuses) = if partners.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        tokio::try_join!(
            client.users_by_ids(&partners),
            client.statuses_by_ids(&partners)
        )?
    };

    let mut store = store.write().expect("store lock");
    store.me = me.id.clone();
    store.add_users(vec![me]);
    store.set_config(config);
    store.set_license(license);
    store.set_preferences(preferences);
    store.set_teams(teams, team_members);
    store.set_team_unreads(unreads);
    store.set_channels(channels, members);
    store.categories.clear();
    for (team, ordered) in categories {
        store.set_categories(team, ordered);
    }
    store.set_roles(roles);
    store.add_users(profiles);
    store.set_statuses(statuses);
    Ok(())
}

/// Brings loaded timelines up to date after time without events: for each channel whose
/// newest post is after `synced_through` (minus a margin), `GET /channels/{id}/posts?since=`.
/// A reply at the cap loads the newest page instead, which leaves a hole for backfill.
pub(crate) async fn catch_up(
    client: &Client,
    store: &SharedStore,
    synced_through: i64,
) -> Result<Vec<StoreDelta>, ApiError> {
    let since = (synced_through - SINCE_MARGIN).max(0);
    let (channels, crt): (Vec<ChannelId>, bool) = {
        let store = store.read().expect("store lock");
        let channels = store
            .timelines
            .iter()
            .filter(|(_, timeline)| timeline.has_recent())
            .filter_map(|(id, _)| store.channels.get(id))
            .filter(|channel| channel.last_post_at >= since)
            .map(|channel| channel.id.clone())
            .collect();
        (channels, store.crt_enabled())
    };
    let mut deltas = Vec::new();
    for channel in channels {
        let list = client
            .channel_posts(
                &channel,
                &PostsQuery {
                    since: Some(since),
                    collapsed_threads: crt,
                    collapsed_threads_extended: crt,
                    ..PostsQuery::default()
                },
            )
            .await?;
        let complete = list.order.len() < SINCE_CAP;
        deltas.extend(store.write().expect("store lock").add_posts(
            &channel,
            &list,
            PageRequest::Since { complete },
        ));
        if !complete {
            let newest = client
                .channel_posts(
                    &channel,
                    &PostsQuery {
                        per_page: Some(PAGE),
                        collapsed_threads: crt,
                        collapsed_threads_extended: crt,
                        ..PostsQuery::default()
                    },
                )
                .await?;
            deltas.extend(store.write().expect("store lock").add_posts(
                &channel,
                &newest,
                PageRequest::Newest,
            ));
        }
    }
    Ok(deltas)
}
