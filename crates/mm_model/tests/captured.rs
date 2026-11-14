// SPDX-License-Identifier: AGPL-3.0-only

//! Responses and WebSocket frames captured from the test server (Mattermost 12.2.0, seeded by
//! `script/test-server/seed.sh`, as `alice`), see `fixtures/README.md`. Every capture must
//! decode; the asserts check what the seed put there (`docs/MatterLite-TestServer.md` §5).

use mm_model::ws::{EventFrame, ServerFrame, WsEvent};
use mm_model::{
    Channel, ChannelMember, ChannelType, ClientConfig, CollapsedThreads, Emoji,
    OrderedSidebarCategories, PostList, Preference, Role, ServerVersion, Status, Team, TeamMember,
    TeamUnread, ThreadList, User,
};
use serde::de::DeserializeOwned;

macro_rules! captured {
    ($name:literal) => {
        include_str!(concat!("../fixtures/captured/", $name))
    };
}

fn decode<T: DeserializeOwned>(name: &str, text: &str) -> T {
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn client_config_and_license() {
    let config: ClientConfig = decode("config_client.json", captured!("config_client.json"));
    assert_eq!(config.site_url(), Some("http://ubuntu:8065"));
    assert_eq!(config.collapsed_threads(), CollapsedThreads::AlwaysOn);
    let version = ServerVersion::parse(config.version().unwrap()).unwrap();
    assert!(version.at_least(12, 2, 0), "{version}");

    let license: ClientConfig = decode("license_client.json", captured!("license_client.json"));
    assert!(!license.bool("IsLicensed"));
}

#[test]
fn me_preferences_teams() {
    let me: User = decode("user_me.json", captured!("user_me.json"));
    assert_eq!(me.username, "alice");
    assert_eq!(me.role_names().collect::<Vec<_>>(), ["system_user"]);

    let preferences: Vec<Preference> = decode("preferences.json", captured!("preferences.json"));
    assert!(!preferences.is_empty());

    let teams: Vec<Team> = decode("teams.json", captured!("teams.json"));
    let mut names: Vec<&str> = teams.iter().map(|team| team.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["matterlite", "second"]);
    let members: Vec<TeamMember> = decode("team_members.json", captured!("team_members.json"));
    assert_eq!(members.len(), 2);
    let unreads: Vec<TeamUnread> = decode("team_unreads.json", captured!("team_unreads.json"));
    assert_eq!(unreads.len(), 2);
}

#[test]
fn channels_memberships_and_categories() {
    let channels: Vec<Channel> = decode("channels.json", captured!("channels.json"));
    for channel_type in [
        ChannelType::Open,
        ChannelType::Private,
        ChannelType::Direct,
        ChannelType::Group,
    ] {
        assert!(
            channels
                .iter()
                .any(|channel| channel.channel_type == channel_type),
            "no {channel_type:?} channel"
        );
    }
    let dev = channels
        .iter()
        .find(|channel| channel.name == "dev")
        .unwrap();

    // NDJSON, as `GET /users/{id}/channel_members?page=-1` streams it.
    let members: Vec<ChannelMember> = captured!("channel_members.ndjson")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| decode("channel_members.ndjson", line))
        .collect();
    // The membership list includes the archived channel; the channel list doesn't.
    assert_eq!(members.len(), channels.len() + 1);

    let categories: OrderedSidebarCategories = decode(
        "sidebar_categories.json",
        captured!("sidebar_categories.json"),
    );
    let favorites = categories
        .categories
        .iter()
        .find(|category| category.category_type == "favorites")
        .unwrap();
    assert_eq!(favorites.channel_ids, std::slice::from_ref(&dev.id));
    assert_eq!(categories.order.len(), categories.categories.len());
}

#[test]
fn dev_posts() {
    let list: PostList = decode("dev_posts.json", captured!("dev_posts.json"));
    assert_eq!(list.order.len(), list.posts.len());
    let posts: Vec<_> = list.ordered().collect();
    let find = |prefix: &str| {
        posts
            .iter()
            .find(|post| post.message.starts_with(prefix))
            .unwrap_or_else(|| panic!("no post starting with {prefix:?}"))
    };

    let showcase = find("# Heading 1");
    assert!(showcase.is_pinned);
    let metadata = showcase.metadata.as_ref().unwrap();
    let mut reactions: Vec<&str> = metadata
        .reactions
        .iter()
        .map(|reaction| reaction.emoji_name.as_str())
        .collect();
    reactions.sort_unstable();
    assert_eq!(reactions, ["+1", "+1", "matterlite", "tada"]);
    assert!(
        metadata
            .emojis
            .iter()
            .any(|emoji| emoji.name == "matterlite"),
        "custom emoji in the metadata"
    );

    let root = find("Thread root");
    assert!(root.is_root());
    assert_eq!(root.reply_count, 3);

    let image = &find("A screenshot").metadata.as_ref().unwrap().files[0];
    assert_eq!(image.name, "picture.png");
    assert_eq!((image.width, image.height), (800, 450));
    assert!(image.has_preview_image);
    assert!(image.mini_preview.is_some());
    assert_eq!(find("Two files").file_ids.len(), 2);

    assert!(find("This message was edited").edit_at > 0);
    assert!(
        posts
            .iter()
            .any(|post| post.props.contains_key("attachments"))
    );
    assert!(
        posts
            .iter()
            .any(|post| post.post_type == "system_add_to_channel")
    );
    // Collapsed threads: the page holds root posts only.
    assert!(posts.iter().all(|post| post.is_root()));
}

#[test]
fn thread_and_threads() {
    let thread: PostList = decode("thread_posts.json", captured!("thread_posts.json"));
    assert_eq!(thread.order.len(), 4);
    let replies = thread.posts.values().filter(|post| !post.is_root()).count();
    assert_eq!(replies, 3);

    let threads: ThreadList = decode("threads.json", captured!("threads.json"));
    assert_eq!(threads.total, 1);
    assert_eq!(threads.total_unread_threads, 1);
    let followed = &threads.threads[0];
    assert!(
        followed
            .post
            .as_ref()
            .unwrap()
            .message
            .starts_with("Thread root")
    );
    assert!(!followed.participants.is_empty());
}

#[test]
fn roles_users_statuses_emoji() {
    let roles: Vec<Role> = decode("roles.json", captured!("roles.json"));
    let system_user = roles
        .iter()
        .find(|role| role.name == "system_user")
        .unwrap();
    assert!(system_user.has_permission("create_direct_channel"));

    let users: Vec<User> = decode("users.json", captured!("users.json"));
    let dave = users.iter().find(|user| user.username == "dave").unwrap();
    assert!(dave.props["customStatus"].contains("palm_tree"));

    let statuses: Vec<Status> = decode("statuses.json", captured!("statuses.json"));
    let mut values: Vec<&str> = statuses
        .iter()
        .map(|status| status.status.as_str())
        .collect();
    values.sort_unstable();
    assert_eq!(values, ["away", "dnd", "offline"]);

    let emoji: Vec<Emoji> = decode("emoji.json", captured!("emoji.json"));
    assert_eq!(emoji[0].name, "matterlite");
}

#[test]
fn websocket_frames_all_decode() {
    let mut events = Vec::new();
    for line in captured!("ws_frames.jsonl").lines() {
        match ServerFrame::parse(line).unwrap_or_else(|error| panic!("{error}: {line}")) {
            ServerFrame::Event(frame) => events.push(frame),
            ServerFrame::Response(response) => assert!(response.is_ok()),
        }
    }
    let seqs: Vec<i64> = events.iter().map(|frame| frame.seq).collect();
    assert_eq!(
        seqs,
        (0..seqs.len() as i64).collect::<Vec<_>>(),
        "in sequence"
    );

    let decoded: Vec<WsEvent> = events
        .iter()
        .map(|frame: &EventFrame| WsEvent::decode(frame).unwrap_or_else(|error| panic!("{error}")))
        .collect();
    assert!(
        !decoded
            .iter()
            .any(|event| matches!(event, WsEvent::Unknown(_))),
        "{decoded:?}"
    );

    let posted = decoded
        .iter()
        .find_map(|event| match event {
            WsEvent::Posted(posted) => Some(posted),
            _ => None,
        })
        .unwrap();
    assert_eq!(posted.sender_name, "@bob");
    assert!(posted.post.message.starts_with("capture:"));

    // A preference change is followed by a `sidebar_category_updated` without data or team.
    let bare_category_updates = decoded
        .iter()
        .zip(&events)
        .filter(|(event, frame)| {
            matches!(event, WsEvent::SidebarCategoryUpdated(categories) if categories.is_empty())
                && frame.broadcast.team_id.is_empty()
        })
        .count();
    assert_eq!(bare_category_updates, 2);
}
