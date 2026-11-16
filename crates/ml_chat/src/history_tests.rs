// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, PostList, User};
use mm_store::PageRequest;
use serde_json::{Value, json};

use super::*;

const ME: &str = "me";
/// 2026-01-10 12:00 UTC.
const NOON: i64 = 1_768_046_400_000;
const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn store(posts: Vec<Value>, reaches_start: bool) -> Store {
    let me: User = serde_json::from_value(json!({
        "id": ME, "username": "me",
        "timezone": {"useAutomaticTimezone": "true", "automaticTimezone": "UTC"},
    }))
    .unwrap();
    let mut store = Store::new(me);
    store.add_users(vec![User {
        id: "bob".into(),
        username: "bob".into(),
        ..User::default()
    }]);
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
        "order": order, "posts": map, "next_post_id": "",
        "prev_post_id": if reaches_start { "" } else { "older" },
    }))
    .unwrap();
    store.add_posts(&"c".into(), &list, PageRequest::Newest);
    store
}

fn post(id: &str, user: &str, at: i64, message: &str) -> Value {
    json!({"id": id, "user_id": user, "create_at": at, "message": message})
}

#[test]
fn days_newest_first_with_labels() {
    let store = store(
        vec![
            post("a", "bob", NOON - 3 * DAY, "old"),
            post("b", "bob", NOON - DAY, "yesterday"),
            post("c", "bob", NOON, "today one"),
            post("d", ME, NOON + HOUR, "today two"),
            json!({"id": "j", "user_id": "bob", "create_at": NOON + 2 * HOUR,
                   "type": "system_join_channel", "props": {"username": "bob"}}),
        ],
        false,
    );
    let clock = Clock::of(&store);
    let today = clock.today(NOON);
    let posts = messages(&store, &"c".into());
    let days = days(&posts, &clock, today);
    let summary: Vec<(String, Vec<&str>)> = days
        .iter()
        .map(|day| {
            (
                day.label.clone(),
                day.posts.iter().map(|post| post.id.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Today".to_owned(), vec!["j", "d", "c"]),
            ("Yesterday".to_owned(), vec!["b"]),
            ("Wed 7 Jan 2026".to_owned(), vec!["a"]),
        ]
    );
    assert_eq!(
        coverage(&store, &"c".into(), &clock).as_deref(),
        Some("Local history: 7 Jan 2026 → now")
    );
}

#[test]
fn open_days_default_and_saved_changes() {
    let today = NaiveDate::from_ymd_opt(2026, 1, 10).unwrap();
    let yesterday = today.pred_opt().unwrap();
    let older = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
    let mut open = OpenDays::default();
    assert!(open.is_open(today, today) && open.is_open(yesterday, today));
    assert!(!open.is_open(older, today));
    open.toggle(older, today);
    open.toggle(today, today);
    let restored = OpenDays::from_json(&open.to_json());
    assert!(restored.is_open(older, today));
    assert!(!restored.is_open(today, today));
    // Back to the default: nothing stored for it.
    let mut back = restored.clone();
    back.toggle(today, today);
    back.toggle(older, today);
    assert_eq!(back, OpenDays::default());
    // Tomorrow, today's group is yesterday's and still open by default.
    assert!(OpenDays::default().is_open(today, today.succ_opt().unwrap()));
}

#[test]
fn matching_text_files_and_authors() {
    let store = store(
        vec![
            post("a", "bob", NOON, "Deploy is done"),
            json!({"id": "f", "user_id": ME, "create_at": NOON + 1, "message": "see file",
                   "metadata": {"files": [{"id": "x", "name": "deploy-log.txt"}]}}),
            post("c", ME, NOON + 2, "nothing here"),
        ],
        true,
    );
    let posts = messages(&store, &"c".into());
    let ids = |matcher: Matcher| {
        let mut found: Vec<String> = scan(&store, &posts, &matcher)
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        found.sort();
        found
    };
    assert_eq!(
        ids(Matcher::new("deploy", false, false).unwrap()),
        ["a", "f"]
    );
    assert_eq!(ids(Matcher::new("Deploy", false, true).unwrap()), ["a"]);
    assert_eq!(ids(Matcher::new("^nothing", true, false).unwrap()), ["c"]);
    assert_eq!(ids(Matcher::new("bob", false, false).unwrap()), ["a"]);

    assert_eq!(excerpt("  a\n\nb  c ", 10), "a b c");
    assert_eq!(excerpt("abcdefghijkl", 5), "abcde…");
    let clock = Clock::of(&store);
    assert_eq!(
        coverage(&store, &"c".into(), &clock).as_deref(),
        Some("Local history: all of it, since 10 Jan 2026")
    );
}
