// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, PostList, User};
use serde_json::{Value, json};

use super::*;
use crate::store::PageRequest;

const ME: &str = "me";
/// 2026-01-10 12:00 UTC.
const NOON: i64 = 1_768_046_400_000;
const MINUTE: i64 = 60_000;

fn store_with(posts: Vec<Value>, zone: Option<&str>) -> Store {
    let mut timezone = json!({});
    if let Some(zone) = zone {
        timezone = json!({"useAutomaticTimezone": "true", "automaticTimezone": zone});
    }
    let me: User = serde_json::from_value(json!({
        "id": ME, "username": "me", "timezone": timezone
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
    let order: Vec<Value> = posts.iter().rev().map(|post| post["id"].clone()).collect();
    let map: serde_json::Map<String, Value> = posts
        .into_iter()
        .map(|mut post| {
            post["channel_id"] = json!("c");
            (post["id"].as_str().unwrap().to_owned(), post)
        })
        .collect();
    let list: PostList = serde_json::from_value(json!({
        "order": order, "posts": map, "next_post_id": "", "prev_post_id": ""
    }))
    .unwrap();
    store.add_posts(&"c".into(), &list, PageRequest::Newest);
    store
}

fn post(id: &str, user: &str, at: i64) -> Value {
    json!({"id": id, "user_id": user, "create_at": at})
}

fn system(id: &str, kind: &str, at: i64, props: Value) -> Value {
    json!({"id": id, "user_id": "admin", "create_at": at, "type": kind, "props": props})
}

/// The items oldest first, as short strings.
fn describe(items: &[TimelineItem]) -> Vec<String> {
    items
        .iter()
        .rev()
        .map(|item| match item {
            TimelineItem::Date(date) => format!("date {date}"),
            TimelineItem::NewMessages { .. } => "new".to_owned(),
            TimelineItem::Post { post, consecutive } => {
                format!("{}{}", post.id, if *consecutive { "+" } else { "" })
            }
            TimelineItem::UserActivity(posts) => format!(
                "activity {}",
                posts
                    .iter()
                    .map(|post| post.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            TimelineItem::Hole => "hole".to_owned(),
        })
        .collect()
}

#[test]
fn dates_follow_the_users_timezone() {
    // 23:30 UTC on Jan 10 is already Jan 11 in Berlin.
    let posts = vec![
        post("a", "bob", NOON),
        post("b", "bob", NOON + 11 * 60 * MINUTE + 30 * MINUTE),
    ];
    let utc = store_with(posts.clone(), Some("UTC"));
    assert_eq!(
        describe(&utc.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "a", "b"]
    );
    let berlin = store_with(posts, Some("Europe/Berlin"));
    assert_eq!(
        describe(&berlin.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "a", "date 2026-01-11", "b"]
    );
}

#[test]
fn new_messages_line_skips_my_own_posts() {
    let store = store_with(
        vec![
            post("a", "bob", NOON),
            post("mine", ME, NOON + 10 * MINUTE),
            post("b", "bob", NOON + 20 * MINUTE),
        ],
        Some("UTC"),
    );
    let options = ItemOptions {
        last_viewed_at: Some(NOON + MINUTE),
    };
    assert_eq!(
        describe(&store.timeline_items(&"c".into(), &options)),
        ["date 2026-01-10", "a", "mine", "new", "b"]
    );
}

#[test]
fn consecutive_posts_share_a_header() {
    let store = store_with(
        vec![
            post("a", "bob", NOON),
            post("b", "bob", NOON + 4 * MINUTE),
            post("c", "bob", NOON + 10 * MINUTE),
            post("d", "carol", NOON + 11 * MINUTE),
            system("s", "system_header_change", NOON + 12 * MINUTE, json!({})),
            post("e", "carol", NOON + 13 * MINUTE),
        ],
        Some("UTC"),
    );
    assert_eq!(
        describe(&store.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "a", "b+", "c", "d", "s", "e"]
    );
}

#[test]
fn join_leave_posts_are_combined_and_can_be_hidden() {
    let posts = vec![
        system(
            "j1",
            "system_join_channel",
            NOON,
            json!({"username": "bob"}),
        ),
        system(
            "j2",
            "system_add_to_channel",
            NOON + MINUTE,
            json!({"addedUsername": "me"}),
        ),
        system(
            "j3",
            "system_leave_channel",
            NOON + 2 * MINUTE,
            json!({"username": "carol"}),
        ),
        post("p", "bob", NOON + 3 * MINUTE),
        system(
            "j4",
            "system_join_channel",
            NOON + 4 * MINUTE,
            json!({"username": "dave"}),
        ),
    ];
    let mut store = store_with(posts, Some("UTC"));
    assert_eq!(
        describe(&store.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "activity j1,j2,j3", "p", "activity j4"]
    );

    // Hidden by preference, except the one about me.
    store.preferences.insert(
        ("advanced_settings".into(), "join_leave".into()),
        "false".into(),
    );
    assert_eq!(
        describe(&store.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "activity j2", "p"]
    );
}

#[test]
fn holes_break_groups_and_restart_dates() {
    let mut store = store_with(vec![post("new", "bob", NOON + 2 * MINUTE)], Some("UTC"));
    // An older page that isn't joined to the newest one.
    let older: PostList = serde_json::from_value(json!({
        "order": ["old"],
        "posts": {"old": {"id": "old", "user_id": "bob", "channel_id": "c", "create_at": NOON}},
        "next_post_id": "x", "prev_post_id": ""
    }))
    .unwrap();
    store.add_posts(&"c".into(), &older, PageRequest::AroundUnread);
    assert_eq!(
        describe(&store.timeline_items(&"c".into(), &ItemOptions::default())),
        ["date 2026-01-10", "old", "hole", "date 2026-01-10", "new"]
    );
}
