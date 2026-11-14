// SPDX-License-Identifier: AGPL-3.0-only

//! Rebuilding a [`Store`] from the cache before any network call (MatterLite.md §8.7: hydrate,
//! render, then reconcile).

use std::collections::HashMap;
use std::sync::Arc;

use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use mm_model::{
    Channel, ChannelId, ChannelMember, ClientConfig, Emoji, Post, PostId, Role, SidebarCategory,
    Team, TeamId, TeamMember, ThreadResponse, User,
};
use mm_store::{Key, Range, Store, ThreadCounts, Timeline};
use serde::de::DeserializeOwned;

use crate::error::CacheError;
use crate::ops::kv as keys;
use crate::schema;

fn decode<T: DeserializeOwned>(what: &'static str, json: &str) -> Result<T, CacheError> {
    serde_json::from_str(json).map_err(|source| CacheError::Decode { what, source })
}

/// A `kv` value, if set.
pub fn get_kv(conn: &mut SqliteConnection, key: &str) -> Result<Option<String>, CacheError> {
    use schema::kv;
    Ok(kv::table
        .find(key)
        .select(kv::value)
        .first::<String>(conn)
        .optional()?)
}

/// The cached state, or `None` for an empty cache (never synced).
pub fn hydrate(conn: &mut SqliteConnection) -> Result<Option<Store>, CacheError> {
    use schema::*;
    let Some(me) = get_kv(conn, keys::ME)? else {
        return Ok(None);
    };
    let me: User = decode("me", &me)?;
    let mut store = Store::new(me);
    if let Some(config) = get_kv(conn, keys::CONFIG)? {
        store.set_config(decode::<ClientConfig>("config", &config)?);
    }
    if let Some(license) = get_kv(conn, keys::LICENSE)? {
        store.set_license(decode::<ClientConfig>("license", &license)?);
    }
    if let Some(counts) = get_kv(conn, keys::THREAD_COUNTS)? {
        let counts: HashMap<TeamId, [i64; 3]> = decode("thread counts", &counts)?;
        store.thread_counts = counts
            .into_iter()
            .map(|(team, [unread_threads, mentions, urgent_mentions])| {
                (
                    team,
                    ThreadCounts {
                        unread_threads,
                        mentions,
                        urgent_mentions,
                    },
                )
            })
            .collect();
    }

    for json in users::table.select(users::json).load::<String>(conn)? {
        let user: User = decode("user", &json)?;
        // The `me` row may be newer than the users row (both are written together).
        if user.id != store.me {
            store.users.insert(user.id.clone(), Arc::new(user));
        }
    }

    let teams_list: Vec<Team> = teams::table
        .select(teams::json)
        .load::<String>(conn)?
        .iter()
        .map(|json| decode("team", json))
        .collect::<Result<_, _>>()?;
    let team_members_list: Vec<TeamMember> = team_members::table
        .select(team_members::json)
        .load::<String>(conn)?
        .iter()
        .map(|json| decode("team member", json))
        .collect::<Result<_, _>>()?;
    store.set_teams(teams_list, team_members_list);

    let channels_list: Vec<Channel> = channels::table
        .select(channels::json)
        .load::<String>(conn)?
        .iter()
        .map(|json| decode("channel", json))
        .collect::<Result<_, _>>()?;
    let members: Vec<ChannelMember> = channel_members::table
        .select(channel_members::json)
        .load::<String>(conn)?
        .iter()
        .map(|json| decode("channel member", json))
        .collect::<Result<_, _>>()?;
    // Channels without a membership (former chats) stay in the file but not in the store.
    store.set_channels(channels_list, members);

    store.preferences = preferences::table
        .select((preferences::category, preferences::name, preferences::value))
        .load::<(String, String, String)>(conn)?
        .into_iter()
        .map(|(category, name, value)| ((category, name), value))
        .collect();
    for (team, json) in categories::table
        .select((categories::team_id, categories::json))
        .load::<(String, String)>(conn)?
    {
        store.categories.insert(
            TeamId::new(team),
            decode::<Vec<SidebarCategory>>("categories", &json)?,
        );
    }
    store.set_roles(
        roles::table
            .select(roles::json)
            .load::<String>(conn)?
            .iter()
            .map(|json| decode::<Role>("role", json))
            .collect::<Result<_, _>>()?,
    );
    store.add_emoji(
        emoji::table
            .select(emoji::json)
            .load::<String>(conn)?
            .iter()
            .map(|json| decode::<Emoji>("emoji", json))
            .collect::<Result<_, _>>()?,
    );
    for json in threads::table.select(threads::json).load::<String>(conn)? {
        let thread: ThreadResponse = decode("thread", &json)?;
        store.thread_states.insert(thread.id.clone(), thread);
    }

    hydrate_posts(conn, &mut store)?;
    Ok(Some(store))
}

/// Posts of my channels, and the timelines and threads over them.
fn hydrate_posts(conn: &mut SqliteConnection, store: &mut Store) -> Result<(), CacheError> {
    use schema::{posts, timeline_empty, timeline_ranges};
    let crt = store.crt_enabled();
    let mut channel_keys: HashMap<ChannelId, Vec<Key>> = HashMap::new();
    let mut thread_keys: HashMap<PostId, Vec<Key>> = HashMap::new();
    for json in posts::table.select(posts::json).load::<String>(conn)? {
        let post: Post = decode("post", &json)?;
        if !store.channel_members.contains_key(&post.channel_id) {
            continue;
        }
        let key = (post.create_at, post.id.clone());
        if !post.is_root() {
            thread_keys
                .entry(post.root_id.clone())
                .or_default()
                .push(key.clone());
        }
        // Under CRT channel timelines hold root posts only (as `Store::add_posts` builds them).
        if post.is_root() || !crt {
            channel_keys
                .entry(post.channel_id.clone())
                .or_default()
                .push(key);
        }
        store.posts.insert(post.id.clone(), Arc::new(post));
    }

    let mut ranges: HashMap<(String, String), Vec<Range>> = HashMap::new();
    for (kind, id, oldest_at, oldest_id, newest_at, newest_id, recent, start) in
        timeline_ranges::table
            .select((
                timeline_ranges::owner_kind,
                timeline_ranges::owner_id,
                timeline_ranges::oldest_at,
                timeline_ranges::oldest_id,
                timeline_ranges::newest_at,
                timeline_ranges::newest_id,
                timeline_ranges::recent,
                timeline_ranges::start,
            ))
            .load::<(String, String, i64, String, i64, String, bool, bool)>(conn)?
    {
        ranges.entry((kind, id)).or_default().push(Range {
            oldest: (oldest_at, PostId::new(oldest_id)),
            newest: (newest_at, PostId::new(newest_id)),
            recent,
            start,
        });
    }
    let empty: std::collections::HashSet<(String, String)> = timeline_empty::table
        .select((timeline_empty::owner_kind, timeline_empty::owner_id))
        .load::<(String, String)>(conn)?
        .into_iter()
        .collect();

    let channel_ids: Vec<ChannelId> = store.channel_members.keys().cloned().collect();
    for channel in channel_ids {
        let owner = ("channel".to_owned(), channel.to_string());
        let channel_ranges = ranges.remove(&owner).unwrap_or_default();
        let is_empty = empty.contains(&owner);
        if channel_ranges.is_empty() && !is_empty {
            continue;
        }
        let keys = channel_keys.remove(&channel).unwrap_or_default();
        store.timelines.insert(
            channel,
            Timeline::from_parts(keys, channel_ranges, is_empty),
        );
    }
    for ((kind, root), thread_ranges) in ranges {
        if kind != "thread" {
            continue;
        }
        let root = PostId::new(root);
        if !store.posts.contains_key(&root) {
            continue;
        }
        let keys = thread_keys.remove(&root).unwrap_or_default();
        store
            .threads
            .insert(root, Timeline::from_parts(keys, thread_ranges, false));
    }
    Ok(())
}
