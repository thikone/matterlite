// SPDX-License-Identifier: AGPL-3.0-only

//! The cache against the data captured from the test server (`mm_model/fixtures/captured`):
//! a store saved and hydrated again must show the same, also after live events went through
//! the writer thread.

use mm_cache::{Cache, SearchHit, ops_for, snapshot_ops};
use mm_model::ws::{ServerFrame, WsEvent};
use mm_model::{
    Channel, ChannelId, ChannelMember, ClientConfig, OrderedSidebarCategories, PostList,
    Preference, Role, Team, TeamId, TeamMember, User,
};
use mm_store::{ItemOptions, PageRequest, Store};
use serde_json::from_str;

macro_rules! captured {
    ($name:literal) => {
        include_str!(concat!("../../mm_model/fixtures/captured/", $name))
    };
}

fn team(store: &Store, name: &str) -> TeamId {
    store
        .teams
        .values()
        .find(|team| team.name == name)
        .unwrap()
        .id
        .clone()
}

fn channel(store: &Store, name: &str) -> ChannelId {
    store
        .channels
        .values()
        .find(|channel| channel.name == name && !channel.team_id.is_empty())
        .unwrap()
        .id
        .clone()
}

fn bootstrap() -> Store {
    let mut store = Store::new(from_str::<User>(captured!("user_me.json")).unwrap());
    store.set_config(from_str::<ClientConfig>(captured!("config_client.json")).unwrap());
    store.set_license(from_str::<ClientConfig>(captured!("license_client.json")).unwrap());
    store.set_preferences(from_str::<Vec<Preference>>(captured!("preferences.json")).unwrap());
    store.set_teams(
        from_str::<Vec<Team>>(captured!("teams.json")).unwrap(),
        from_str::<Vec<TeamMember>>(captured!("team_members.json")).unwrap(),
    );
    store.set_team_unreads(from_str(captured!("team_unreads.json")).unwrap());
    let members: Vec<ChannelMember> = captured!("channel_members.ndjson")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| from_str(line).unwrap())
        .collect();
    store.set_channels(
        from_str::<Vec<Channel>>(captured!("channels.json")).unwrap(),
        members,
    );
    let matterlite = team(&store, "matterlite");
    store.set_categories(
        matterlite.clone(),
        from_str::<OrderedSidebarCategories>(captured!("sidebar_categories.json")).unwrap(),
    );
    store.set_roles(from_str::<Vec<Role>>(captured!("roles.json")).unwrap());
    store.add_users(from_str::<Vec<User>>(captured!("users.json")).unwrap());
    store.add_emoji(from_str(captured!("emoji.json")).unwrap());
    let dev = channel(&store, "dev");
    store.add_posts(
        &dev,
        &from_str::<PostList>(captured!("dev_posts.json")).unwrap(),
        PageRequest::Newest,
    );
    let root = store
        .posts
        .values()
        .find(|post| post.message.starts_with("Thread root"))
        .unwrap()
        .id
        .clone();
    store.add_thread(
        &root,
        &from_str::<PostList>(captured!("thread_posts.json")).unwrap(),
    );
    store.set_threads(&matterlite, from_str(captured!("threads.json")).unwrap());
    store
}

/// Everything the UI reads, compared between two stores.
fn assert_same_view(expected: &Store, actual: &Store) {
    assert_eq!(actual.me, expected.me);
    assert_eq!(actual.config, expected.config);
    assert_eq!(actual.users, expected.users);
    assert_eq!(actual.teams, expected.teams);
    assert_eq!(actual.team_members, expected.team_members);
    assert_eq!(actual.thread_counts, expected.thread_counts);
    assert_eq!(actual.channels, expected.channels);
    assert_eq!(actual.channel_members, expected.channel_members);
    assert_eq!(actual.preferences, expected.preferences);
    assert_eq!(actual.categories, expected.categories);
    assert_eq!(actual.roles, expected.roles);
    assert_eq!(actual.emoji, expected.emoji);
    assert_eq!(actual.thread_states, expected.thread_states);
    assert_eq!(actual.posts, expected.posts);
    for team in expected.teams.keys() {
        assert_eq!(actual.team_unread(team), expected.team_unread(team));
        assert_eq!(actual.sidebar(team, None), expected.sidebar(team, None));
    }
    let options = ItemOptions {
        last_viewed_at: Some(1),
    };
    for (id, timeline) in &expected.timelines {
        assert_eq!(
            actual.timeline_items(id, &options),
            expected.timeline_items(id, &options),
            "channel {id}"
        );
        assert_eq!(
            actual.timelines[id].ranges(),
            timeline.ranges(),
            "channel {id}"
        );
    }
    for (root, thread) in &expected.threads {
        if thread.ranges().is_empty() {
            continue;
        }
        let ids: Vec<_> = thread.post_ids().collect();
        assert_eq!(actual.threads[root].post_ids().collect::<Vec<_>>(), ids);
    }
}

#[test]
fn a_snapshot_hydrates_to_the_same_view() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("servers/abc/cache.sqlite");
    let store = bootstrap();
    {
        let (mut cache, report) = Cache::open(&path).unwrap();
        assert!(report.moved_aside.is_none());
        assert!(cache.hydrate().unwrap().is_none(), "empty cache");
        cache.apply(&snapshot_ops(&store)).unwrap();
    }
    let (mut cache, _) = Cache::open(&path).unwrap();
    let hydrated = cache.hydrate().unwrap().unwrap();
    assert_same_view(&store, &hydrated);
}

#[test]
fn live_events_through_the_writer_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let mut store = bootstrap();
    let (mut cache, _) = Cache::open(&path).unwrap();
    cache.apply(&snapshot_ops(&store)).unwrap();
    let writer = cache.into_writer().unwrap();
    for line in captured!("ws_frames.jsonl").lines() {
        if let ServerFrame::Event(frame) = ServerFrame::parse(line).unwrap() {
            let event = WsEvent::decode(&frame).unwrap();
            let deltas = store.apply(&event, &frame.broadcast);
            writer.send(ops_for(&store, &deltas)).unwrap();
        }
    }
    writer.close().unwrap();

    let (mut cache, _) = Cache::open(&path).unwrap();
    let hydrated = cache.hydrate().unwrap().unwrap();
    assert_same_view(&store, &hydrated);
    // The captured run deleted its root post: neither it nor its replies are cached.
    assert!(cache.search("capture", None, 10).unwrap().is_empty());
}

#[test]
fn search_finds_messages_files_and_attachments() {
    let dir = tempfile::tempdir().unwrap();
    let store = bootstrap();
    let (mut cache, _) = Cache::open(&dir.path().join("cache.sqlite")).unwrap();
    cache.apply(&snapshot_ops(&store)).unwrap();
    let dev = channel(&store, "dev");
    let first_lines = |hits: &[SearchHit]| -> Vec<String> {
        hits.iter()
            .map(|hit| {
                store.posts[&hit.post]
                    .message
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .collect()
    };

    let hits = cache.search("tag the rel", None, 10).unwrap();
    assert_eq!(
        first_lines(&hits),
        ["Agreed. @bob can you tag the release?"]
    );
    // File names.
    let hits = cache.search("picture.png", Some(&dev), 10).unwrap();
    assert_eq!(first_lines(&hits), ["A screenshot"]);
    // The last word is a prefix; any case.
    assert!(!cache.search("MARKD", None, 10).unwrap().is_empty());
    // Channel filter.
    let other = channel(&store, "town-square");
    assert!(
        cache
            .search("picture.png", Some(&other), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_broken_file_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    std::fs::write(&path, b"this is not a database, just bytes that look wrong").unwrap();
    let (mut cache, report) = Cache::open(&path).unwrap();
    let aside = report.moved_aside.expect("moved aside");
    assert!(aside.exists());
    let name = aside.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with("cache.sqlite.broken-"), "{name}");
    assert!(cache.hydrate().unwrap().is_none());
}

#[test]
fn account_ids_are_stable_and_separate_users() {
    let alice = mm_cache::account_id("https://chat.example.com", &"alice".into());
    assert_eq!(alice.len(), 16);
    assert_eq!(
        alice,
        mm_cache::account_id("https://chat.example.com/", &"alice".into())
    );
    assert_ne!(
        alice,
        mm_cache::account_id("https://chat.example.com", &"bob".into())
    );
}

#[test]
fn a_reader_works_while_the_writer_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let store = bootstrap();
    let (cache, _) = Cache::open(&path).unwrap();
    let writer = cache.into_writer().unwrap();
    writer.send(snapshot_ops(&store)).unwrap();
    writer.flush().unwrap();
    let mut reader = mm_cache::open_reader(&path).unwrap();
    assert_eq!(
        mm_cache::search(&mut reader, "picture.png", None, 10)
            .unwrap()
            .len(),
        1
    );
    // More writes while the reader stays open.
    let mut post = store
        .posts
        .values()
        .find(|post| post.message == "A screenshot")
        .unwrap()
        .as_ref()
        .clone();
    post.message = "A screenshot, renamed to diagram".into();
    writer.send(vec![mm_cache::Op::PutPost(post)]).unwrap();
    writer.flush().unwrap();
    assert_eq!(
        mm_cache::search(&mut reader, "diagram", None, 10)
            .unwrap()
            .len(),
        1
    );
    writer.close().unwrap();
}

/// 20 000 posts in one channel through the writer, then hydrated (timings with
/// `--nocapture`).
#[test]
fn a_large_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let mut store = bootstrap();
    let dev = channel(&store, "dev");
    let template = store
        .posts
        .values()
        .find(|post| post.channel_id == dev && post.post_type.is_empty())
        .unwrap()
        .as_ref()
        .clone();
    let posts: std::collections::HashMap<_, _> = (0..20_000)
        .map(|n| {
            let mut post = template.clone();
            post.id = format!("bulk{n:022}").into();
            post.create_at = 1_000_000 + n;
            post.message = format!("bulk message number {n} about release planning");
            post.metadata = None;
            (post.id.clone(), post)
        })
        .collect();
    let order = {
        let mut ids: Vec<_> = posts.keys().cloned().collect();
        ids.sort_by_key(|id| std::cmp::Reverse(posts[id].create_at));
        ids
    };
    let list = PostList {
        order,
        posts,
        ..PostList::default()
    };
    let start = std::time::Instant::now();
    let deltas = store.add_posts(&dev, &list, PageRequest::Before(list.order[0].clone()));
    let (cache, _) = Cache::open(&path).unwrap();
    let writer = cache.into_writer().unwrap();
    writer.send(snapshot_ops(&store)).unwrap();
    writer.send(ops_for(&store, &deltas)).unwrap();
    writer.close().unwrap();
    let written = start.elapsed();

    let start = std::time::Instant::now();
    let (mut cache, _) = Cache::open(&path).unwrap();
    let hydrated = cache.hydrate().unwrap().unwrap();
    let hydrate = start.elapsed();
    assert!(hydrated.posts.len() > 20_000);
    let start = std::time::Instant::now();
    let hits = cache.search("release plan", Some(&dev), 50).unwrap();
    let search = start.elapsed();
    assert_eq!(hits.len(), 50);
    let size = std::fs::metadata(&path).unwrap().len()
        + std::fs::metadata(path.with_extension("sqlite-wal")).map_or(0, |m| m.len());
    println!(
        "20k posts: write {written:?}, hydrate {hydrate:?}, search {search:?}, file {} KB",
        size / 1024
    );
}
