// SPDX-License-Identifier: AGPL-3.0-only

//! The store built from responses captured from the test server (`mm_model/fixtures/captured`),
//! checked against the server's own numbers, then fed the captured WebSocket frames.

use mm_model::ws::{ServerFrame, WsEvent};
use mm_model::{
    Channel, ChannelId, ChannelMember, ClientConfig, OrderedSidebarCategories, PostList,
    Preference, Role, Status, Team, TeamId, TeamMember, User,
};
use mm_store::{PageRequest, Store, StoreDelta};
use serde::de::DeserializeOwned;

macro_rules! captured {
    ($name:literal) => {
        include_str!(concat!("../../mm_model/fixtures/captured/", $name))
    };
}

fn decode<T: DeserializeOwned>(text: &str) -> T {
    serde_json::from_str(text).unwrap()
}

fn team_id(store: &Store, name: &str) -> TeamId {
    store
        .teams
        .values()
        .find(|team| team.name == name)
        .unwrap()
        .id
        .clone()
}

fn channel_id(store: &Store, name: &str) -> ChannelId {
    store
        .channels
        .values()
        .find(|channel| channel.name == name && !channel.team_id.is_empty())
        .unwrap()
        .id
        .clone()
}

fn bootstrap() -> Store {
    let mut store = Store::new(decode::<User>(captured!("user_me.json")));
    store.set_config(decode::<ClientConfig>(captured!("config_client.json")));
    store.set_license(decode::<ClientConfig>(captured!("license_client.json")));
    store.set_preferences(decode::<Vec<Preference>>(captured!("preferences.json")));
    store.set_teams(
        decode::<Vec<Team>>(captured!("teams.json")),
        decode::<Vec<TeamMember>>(captured!("team_members.json")),
    );
    store.set_team_unreads(decode(captured!("team_unreads.json")));
    let members: Vec<ChannelMember> = captured!("channel_members.ndjson")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(decode)
        .collect();
    store.set_channels(decode::<Vec<Channel>>(captured!("channels.json")), members);
    let matterlite = team_id(&store, "matterlite");
    store.set_categories(
        matterlite,
        decode::<OrderedSidebarCategories>(captured!("sidebar_categories.json")),
    );
    store.set_roles(decode::<Vec<Role>>(captured!("roles.json")));
    store.add_users(decode::<Vec<User>>(captured!("users.json")));
    store.set_statuses(decode::<Vec<Status>>(captured!("statuses.json")));
    let dev = channel_id(&store, "dev");
    store.add_posts(
        &dev,
        &decode::<PostList>(captured!("dev_posts.json")),
        PageRequest::Newest,
    );
    store
}

#[test]
fn team_badges_match_the_server() {
    let store = bootstrap();
    assert!(store.crt_enabled());
    // `/users/me/teams/unread` at capture time (root counters under CRT).
    for unread in decode::<Vec<mm_model::TeamUnread>>(captured!("team_unreads.json")) {
        let badge = store.team_unread(&unread.team_id);
        assert_eq!(badge.messages, unread.msg_count_root, "{}", unread.team_id);
        assert_eq!(badge.mentions, unread.mention_count_root);
        assert_eq!(badge.threads, unread.thread_count);
        assert_eq!(badge.thread_mentions, unread.thread_mention_count);
    }
    // Every DM and GM message counts as a mention (MatterLite-TestServer.md §6).
    let direct = store.direct_unread();
    assert_eq!(direct.messages, direct.mentions);
    assert!(direct.mentions > 0);
}

#[test]
fn names_permissions_and_the_dev_timeline() {
    let store = bootstrap();
    let mut names: Vec<String> = store
        .channels
        .values()
        .filter(|channel| channel.channel_type.is_direct_or_group())
        .map(|channel| store.channel_name(&channel.id).unwrap())
        .collect();
    names.sort();
    // testbot's profile wasn't captured, so its id stands in.
    let testbot = names.iter().position(|name| name.len() == 26).unwrap();
    names.remove(testbot);
    assert_eq!(names, ["alice", "bob", "bob, carol", "carol"]);

    let dev = channel_id(&store, "dev");
    assert!(store.has_channel_permission(&dev, "create_post"));
    assert!(!store.has_channel_permission(&dev, "manage_system"));

    // The captured page was the one before the live-test posts: it reaches the start, not
    // the newest post, so it's shown below a hole.
    let shown = store.timeline_posts(&dev);
    assert!(shown[0].is_none());
    assert_eq!(shown.iter().flatten().count(), 16);
    assert!(shown.iter().flatten().all(|post| post.is_root()));
}

#[test]
fn captured_events_apply_cleanly() {
    let mut store = bootstrap();
    let dev = channel_id(&store, "dev");
    let empty = channel_id(&store, "empty");
    let before = store.channels[&dev].as_ref().clone();
    let mention_before = store.channel_members[&dev].mention_count_root;

    let mut deltas = Vec::new();
    for line in captured!("ws_frames.jsonl").lines() {
        if let ServerFrame::Event(frame) = ServerFrame::parse(line).unwrap() {
            let event = WsEvent::decode(&frame).unwrap();
            deltas.extend(store.apply(&event, &frame.broadcast));
        }
    }

    // Bob's root and two replies counted; the root mentioned alice.
    let after = &store.channels[&dev];
    assert_eq!(after.total_msg_count, before.total_msg_count + 3);
    assert_eq!(after.total_msg_count_root, before.total_msg_count_root + 1);
    assert_eq!(
        store.channel_members[&dev].mention_count_root,
        mention_before + 1
    );
    // The root was deleted at the end, its replies with it.
    assert!(
        !store
            .posts
            .values()
            .any(|post| post.message.starts_with("capture:"))
    );
    // Mute toggled back, category expanded again, the test preference gone.
    assert!(!store.channel_members[&empty].is_muted());
    let channels_category = store.categories[&team_id(&store, "matterlite")]
        .iter()
        .find(|category| category.category_type == "channels")
        .unwrap();
    assert!(!channels_category.collapsed);
    assert!(store.preference("matterlite_capture", "test").is_none());
    // Bob typed, and nothing needed fetching.
    assert!(
        deltas
            .iter()
            .any(|delta| matches!(delta, StoreDelta::Typing { .. }))
    );
    assert!(
        !deltas
            .iter()
            .any(|delta| matches!(delta, StoreDelta::Fetch(_)))
    );
}
