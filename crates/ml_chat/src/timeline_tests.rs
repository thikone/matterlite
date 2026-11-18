// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, PostList};
use mm_store::PageRequest;
use serde_json::{Value, json};

use super::*;

const ME: &str = "me";
/// 2026-01-10 12:00 UTC.
const NOON: i64 = 1_768_046_400_000;
const MINUTE: i64 = 60_000;

fn store() -> Store {
    let me: User = serde_json::from_value(json!({
        "id": ME, "username": "me", "first_name": "Maria",
        "timezone": {"useAutomaticTimezone": "true", "automaticTimezone": "UTC"},
        "notify_props": {"first_name": "true", "mention_keys": "deploy,@oncall", "channel": "true"},
    }))
    .unwrap();
    let mut store = Store::new(me);
    store.set_channels(
        vec![Channel {
            id: "c".into(),
            ..Channel::default()
        }],
        vec![ChannelMember {
            channel_id: "c".into(),
            user_id: ME.into(),
            ..ChannelMember::default()
        }],
    );
    store
}

fn page(store: &mut Store, posts: Vec<Value>, request: PageRequest, prev: &str, next: &str) {
    let order: Vec<Value> = posts.iter().rev().map(|post| post["id"].clone()).collect();
    let map: serde_json::Map<String, Value> = posts
        .into_iter()
        .map(|mut post| {
            post["channel_id"] = json!("c");
            (post["id"].as_str().unwrap().to_owned(), post)
        })
        .collect();
    let list: PostList = serde_json::from_value(json!({
        "order": order, "posts": map, "next_post_id": next, "prev_post_id": prev
    }))
    .unwrap();
    store.add_posts(&"c".into(), &list, request);
}

fn post(id: &str, user: &str, at: i64) -> Value {
    json!({"id": id, "user_id": user, "create_at": at, "message": id})
}

fn activity(kind: &str, props: Value) -> Arc<Post> {
    Arc::new(serde_json::from_value(json!({"type": kind, "props": props})).unwrap())
}

fn describe(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Start => "start".to_owned(),
            Row::Date(date) => format!("date {date}"),
            Row::NewMessages => "new".to_owned(),
            Row::Post {
                post, consecutive, ..
            } => format!("{}{}", post.id, if *consecutive { "+" } else { "" }),
            Row::Activity { text, .. } => format!("activity {text}"),
            Row::Hole(kind) => format!("hole {kind:?}"),
            Row::ReplyCount(count) => replies_label(*count),
        })
        .collect()
}

#[test]
fn rows_oldest_first_with_the_start_and_holes() {
    let mut store = store();
    // The whole history: the start row.
    page(
        &mut store,
        vec![post("a", "bob", NOON), post("b", "bob", NOON + MINUTE)],
        PageRequest::Newest,
        "",
        "",
    );
    assert_eq!(
        describe(&rows(&store, &"c".into(), None)),
        ["start", "date 2026-01-10", "a", "b+"]
    );

    // Only the newest page: older history can be loaded.
    let mut store = self::store();
    page(
        &mut store,
        vec![post("y", "bob", NOON), post("z", "carol", NOON + MINUTE)],
        PageRequest::Newest,
        "x",
        "",
    );
    let rows = rows(&store, &"c".into(), None);
    assert_eq!(describe(&rows), ["hole Older", "date 2026-01-10", "y", "z"]);
    assert_eq!(rows[0].key(rows.get(1)), RowKey::Hole(None));
    assert_eq!(rows[2].key(rows.get(3)), RowKey::Post("y".into()));
}

#[test]
fn opened_at_the_first_unread_post_loads_both_ways() {
    let mut store = store();
    page(
        &mut store,
        vec![post("m", "bob", NOON), post("n", "bob", NOON + 10 * MINUTE)],
        PageRequest::AroundUnread,
        "l",
        "o",
    );
    assert_eq!(
        describe(&rows(&store, &"c".into(), Some(NOON + MINUTE))),
        [
            "hole Older",
            "date 2026-01-10",
            "m",
            "new",
            "n",
            "hole Newer"
        ]
    );
}

#[test]
fn combined_activity_reads_like_the_webapp() {
    let joined = |name: &str| activity("system_join_channel", json!({"username": name}));
    let left = |name: &str| activity("system_leave_channel", json!({"username": name}));
    let added = |by: &str, name: &str| {
        activity(
            "system_add_to_channel",
            json!({"username": by, "addedUsername": name}),
        )
    };
    let removed = |name: &str| {
        activity(
            "system_remove_from_channel",
            json!({"username": "admin", "removedUsername": name}),
        )
    };
    assert_eq!(
        activity_markdown(&[joined("bob"), joined("carol"), joined("dave")], "me"),
        "@bob, @carol and @dave **joined the channel**."
    );
    assert_eq!(
        activity_markdown(&[joined("bob"), left("bob")], "me"),
        "@bob **joined and left the channel**."
    );
    assert_eq!(
        activity_markdown(&[added("alice", "bob"), added("alice", "carol")], "me"),
        "@bob and @carol **added to the channel** by @alice."
    );
    assert_eq!(
        activity_markdown(&[added("alice", "me")], "me"),
        "You were **added to the channel** by @alice."
    );
    assert_eq!(
        activity_markdown(&[removed("bob"), joined("erin")], "me"),
        "@bob was **removed from the channel**. @erin **joined the channel**."
    );
    assert_eq!(
        activity_markdown(&[removed("bob"), removed("carol")], "me"),
        "@bob and @carol were **removed from the channel**."
    );
}

#[test]
fn times_and_dates() {
    let utc = Clock::of(&store());
    assert!(matches!(utc.zone, Zone::Named(_)));
    assert_eq!(utc.time(NOON + 2 * MINUTE), "12:02 PM");
    assert_eq!(utc.time(NOON - 12 * 60 * MINUTE + 5 * MINUTE), "12:05 AM");
    let military = Clock {
        military: true,
        ..utc
    };
    assert_eq!(military.time(NOON + 9 * 60 * MINUTE), "21:00");

    let today = NaiveDate::from_ymd_opt(2026, 1, 10).unwrap();
    assert_eq!(date_label(today, today), "Today");
    assert_eq!(date_label(today.pred_opt().unwrap(), today), "Yesterday");
    assert_eq!(
        date_label(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(), today),
        "Monday, January 5"
    );
    assert_eq!(
        date_label(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap(), today),
        "December 31, 2025"
    );
}

#[test]
fn authors_and_the_render_context() {
    let mut store = store();
    store.add_users(vec![
        serde_json::from_value(json!({
            "id": "bob", "username": "bob", "first_name": "Bob", "last_name": "Stone",
            "last_picture_update": 7
        }))
        .unwrap(),
        serde_json::from_value(json!({"id": "hook", "username": "hookbot", "is_bot": true}))
            .unwrap(),
    ]);
    let normal: Post = serde_json::from_value(json!({"user_id": "bob"})).unwrap();
    let author = author(&store, &normal);
    assert_eq!(author.name, "bob");
    assert_eq!(author.initials, "BS");
    assert!(!author.bot);
    assert_eq!(
        author.picture,
        Some(ImageKey::Avatar {
            user: "bob".into(),
            updated: 7
        })
    );
    let webhook: Post = serde_json::from_value(json!({
        "user_id": "hook",
        "props": {"from_webhook": "true", "override_username": "CI"}
    }))
    .unwrap();
    // The override is used only when the server allows it.
    assert_eq!(super::author(&store, &webhook).name, "hookbot");
    store
        .set_config(serde_json::from_value(json!({"EnablePostUsernameOverride": "true"})).unwrap());
    let webhook = super::author(&store, &webhook);
    assert_eq!((webhook.name.as_str(), webhook.bot), ("CI", true));
    assert_eq!(webhook.picture, None);

    store.set_channels(
        vec![
            serde_json::from_value(json!({"id": "c", "team_id": "t", "name": "dev",
                                          "display_name": "Dev"}))
            .unwrap(),
            serde_json::from_value(json!({"id": "o", "team_id": "other", "name": "ops"})).unwrap(),
        ],
        vec![
            serde_json::from_value(json!({"channel_id": "c", "user_id": ME})).unwrap(),
            serde_json::from_value(json!({"channel_id": "o", "user_id": ME})).unwrap(),
        ],
    );
    let lookup = ChatLookup::new(&store, &"c".into(), Some(&"t".into()));
    let context = lookup.context();
    assert_eq!(context.my_username, "me");
    assert_eq!(context.mention_keys, ["maria", "deploy", "oncall"]);
    assert!(context.channel_mentions_notify);
    assert_eq!(lookup.user("bob").map(|user| user.id), Some("bob".into()));
    assert_eq!(lookup.channel_id("dev"), Some(&"c".into()));
    assert_eq!(lookup.channel_id("ops"), None, "another team's channel");
}

#[test]
fn up_edits_my_newest_post_that_can_be_interacted_with() {
    let post = |id: &str, user: &str, extra: Value| -> Post {
        let mut value = json!({"id": id, "user_id": user, "channel_id": "c"});
        if let (Some(value), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            value.extend(extra.clone());
        }
        serde_json::from_value(value).unwrap()
    };
    let posts = [
        (post("old", ME, json!({})), false),
        (post("mine", ME, json!({"edit_at": 5})), false),
        (post("theirs", "bob", json!({})), false),
        (
            post("joined", ME, json!({"type": "system_join_channel"})),
            false,
        ),
        (post("gone", ME, json!({"delete_at": 9})), false),
        (post("me:123", ME, json!({})), true),
    ];
    let me = UserId::from(ME);
    let latest = latest_to_edit(posts.iter().map(|(post, pending)| (post, *pending)), &me);
    assert_eq!(latest, Some(PostId::from("mine")));
    assert_eq!(
        latest_to_edit(posts[2..].iter().map(|(p, b)| (p, *b)), &me),
        None
    );
}

#[test]
fn reaction_tooltips_name_who_reacted() {
    let others = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        reaction_tooltip(true, &[], "smile", Some("(click to remove)")),
        "You reacted with :smile: (click to remove)"
    );
    assert_eq!(
        reaction_tooltip(false, &others(&["bob"]), "+1", Some("(click to add)")),
        "bob reacted with :+1: (click to add)"
    );
    assert_eq!(
        reaction_tooltip(true, &others(&["bob", "carol"]), "tada", None),
        "You, bob and carol reacted with :tada:"
    );
    let many: Vec<String> = (0..22).map(|i| format!("u{i}")).collect();
    let text = reaction_tooltip(false, &many, "x", None);
    assert!(
        text.ends_with("u19 and 2 other users reacted with :x:"),
        "{text}"
    );
}

#[test]
fn reaction_chips_group_by_emoji_in_order() {
    let store = store();
    let post: Post = serde_json::from_value(json!({
        "id": "p", "channel_id": "c",
        "metadata": {"reactions": [
            {"user_id": "bob", "post_id": "p", "emoji_name": "tada", "create_at": 3},
            {"user_id": ME, "post_id": "p", "emoji_name": "smile", "create_at": 1},
            {"user_id": "carol", "post_id": "p", "emoji_name": "smile", "create_at": 2},
        ]}
    }))
    .unwrap();
    let chips = reaction_chips(&store, &post, true, true);
    let summary: Vec<(&str, usize, bool)> = chips
        .iter()
        .map(|chip| (chip.name.as_str(), chip.count, chip.mine))
        .collect();
    assert_eq!(summary, [("smile", 2, true), ("tada", 1, false)]);
    assert!(chips[0].tooltip.starts_with("You and "));
    assert!(chips[0].tooltip.ends_with("(click to remove)"));
    assert!(chips[1].tooltip.ends_with("(click to add)"));
    let read_only = reaction_chips(&store, &post, false, false);
    assert!(read_only[1].tooltip.ends_with(":tada:"));
}

#[test]
fn thread_rows_show_the_root_the_count_and_the_replies() {
    let mut store = store();
    let root = PostId::from("root");
    // Not loaded at all, then only the root.
    assert_eq!(thread_rows(&store, &root), [Row::Hole(HoleKind::Replies)]);
    let post = |id: &str, root: &str, user: &str, at: i64| {
        json!({"id": id, "channel_id": "c", "root_id": root, "user_id": user, "create_at": at,
               "message": id})
    };
    page(
        &mut store,
        vec![post("root", "", "bob", NOON)],
        PageRequest::Newest,
        "",
        "",
    );
    let rows = thread_rows(&store, &root);
    assert!(matches!(rows.last(), Some(Row::Hole(HoleKind::Replies))));

    let list: mm_model::PostList = serde_json::from_value(json!({
        "order": ["r3", "r2", "r1", "root"],
        "posts": {
            "root": post("root", "", "bob", NOON),
            "r1": post("r1", "root", ME, NOON + MINUTE),
            "r2": post("r2", "root", ME, NOON + 2 * MINUTE),
            "r3": post("r3", "root", "bob", NOON + 24 * 60 * MINUTE),
        },
        "next_post_id": "", "prev_post_id": ""
    }))
    .unwrap();
    store.add_thread(&root, &list);
    let rows = thread_rows(&store, &root);
    let summary: Vec<String> = rows
        .iter()
        .map(|row| match row {
            Row::Date(date) => format!("date {date}"),
            Row::Post {
                post, consecutive, ..
            } => format!("{}{}", post.id, if *consecutive { "+" } else { "" }),
            Row::ReplyCount(count) => replies_label(*count),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        summary,
        [
            "date 2026-01-10",
            "root",
            "3 replies",
            "r1",
            "r2+",
            "date 2026-01-11",
            "r3",
        ]
    );
}

#[test]
fn edited_labels_say_when() {
    // The test user's zone is UTC.
    let clock = Clock::of(&store());
    assert_eq!(
        edited_label(&clock, NOON + 2 * MINUTE, NOON + 60 * MINUTE),
        "Edited today at 12:02 PM"
    );
    assert_eq!(
        edited_label(&clock, NOON - 24 * 60 * MINUTE, NOON),
        "Edited yesterday at 12:00 PM"
    );
    assert_eq!(
        edited_label(&clock, NOON - 5 * 24 * 60 * MINUTE, NOON),
        "Edited Monday, January 5 at 12:00 PM"
    );
}
