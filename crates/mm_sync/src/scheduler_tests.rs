// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, OrderedSidebarCategories, User};
use serde_json::json;

use super::*;

const DAY: i64 = 24 * 60 * 60 * 1000;
const NOW: i64 = 1_800_000_000_000;

/// Channels: `mention` (a mention), `unread`, `fav` (favourite, read), `dm` (read DM),
/// `seen` (read, viewed recently), `old` (read, viewed long ago), `gone` (archived).
fn store() -> Store {
    let mut store = Store::new(User {
        id: "me".into(),
        ..User::default()
    });
    let channel = |id: &str, kind: &str| -> Channel {
        serde_json::from_value(json!({
            "id": id, "type": kind, "team_id": if kind == "D" { "" } else { "t1" },
            "name": if kind == "D" { "me__bob" } else { id },
            "total_msg_count": 5, "total_msg_count_root": 5,
            "delete_at": if id == "gone" { 1 } else { 0 },
        }))
        .unwrap()
    };
    let channels = vec![
        channel("old", "O"),
        channel("seen", "O"),
        channel("dm", "D"),
        channel("fav", "O"),
        channel("unread", "O"),
        channel("mention", "O"),
        channel("gone", "O"),
    ];
    let members = channels
        .iter()
        .map(|channel| {
            let id = channel.id.as_str();
            let read = !matches!(id, "unread" | "mention");
            ChannelMember {
                channel_id: channel.id.clone(),
                user_id: "me".into(),
                msg_count: if read { 5 } else { 0 },
                msg_count_root: if read { 5 } else { 0 },
                mention_count_root: i64::from(id == "mention"),
                last_viewed_at: match id {
                    "seen" => 500,
                    "old" => 100,
                    _ => 0,
                },
                ..ChannelMember::default()
            }
        })
        .collect();
    store.set_channels(channels, members);
    store.set_config(serde_json::from_value(json!({"CollapsedThreads": "always_on"})).unwrap());
    let categories: OrderedSidebarCategories = serde_json::from_value(json!({
        "categories": [{"id": "f", "team_id": "t1", "type": "favorites", "channel_ids": ["fav"]}],
        "order": ["f"]
    }))
    .unwrap();
    store.set_categories("t1".into(), categories);
    store
}

fn ids(channels: &[ChannelId]) -> Vec<&str> {
    channels.iter().map(ChannelId::as_str).collect()
}

/// A newest page of `count` posts ending at `newest_at`, one per day going back.
fn load_newest(store: &mut Store, channel: &str, newest_at: i64, count: i64, reaches_start: bool) {
    let posts: serde_json::Map<String, serde_json::Value> = (0..count)
        .map(|n| {
            let id = format!("{channel}{n}");
            (
                id.clone(),
                json!({"id": id, "channel_id": channel, "create_at": newest_at - n * DAY}),
            )
        })
        .collect();
    let order: Vec<String> = (0..count).map(|n| format!("{channel}{n}")).collect();
    let list: PostList = serde_json::from_value(json!({
        "order": order, "posts": posts, "next_post_id": "",
        "prev_post_id": if reaches_start { "" } else { "older" }
    }))
    .unwrap();
    store.add_posts(&channel.into(), &list, PageRequest::Newest);
}

#[test]
fn channels_go_mentions_unread_favourites_dms_recent_rest() {
    let store = store();
    assert_eq!(
        ids(&channel_order(&store)),
        ["mention", "unread", "fav", "dm", "seen", "old"],
        "archived channels are left out"
    );
}

#[test]
fn recent_pages_only_for_channels_without_one() {
    let mut store = store();
    load_newest(&mut store, "unread", NOW, 3, false);
    assert_eq!(
        ids(&plan_recent(&store)),
        ["mention", "fav", "dm", "seen", "old"]
    );
}

#[test]
fn backfill_stops_at_the_window_or_the_start() {
    let mut store = store();
    // `seen`: three days loaded, more on the server.
    load_newest(&mut store, "seen", NOW, 3, false);
    // `old`: everything loaded.
    load_newest(&mut store, "old", NOW, 3, true);
    let anchor = |store: &Store, history| backfill_anchor(store, &"seen".into(), history, NOW);
    assert_eq!(
        anchor(&store, History::Days(180)).unwrap().as_str(),
        "seen2"
    );
    assert_eq!(anchor(&store, History::All).unwrap().as_str(), "seen2");
    // The oldest loaded post (two days old) is already outside a one-day window.
    assert_eq!(anchor(&store, History::Days(1)), None);
    assert_eq!(anchor(&store, History::Off), None);
    assert_eq!(
        backfill_anchor(&store, &"old".into(), History::All, NOW),
        None
    );
    // Channels without a newest page wait for P2.
    assert_eq!(
        plan_backfill(&store, History::All, NOW),
        [("seen".into(), "seen2".into())]
    );
}

#[test]
fn followed_threads_needing_replies() {
    let mut store = store();
    let list: ThreadList = serde_json::from_value(json!({
        "threads": [
            {"id": "fresh", "last_reply_at": NOW - DAY},
            {"id": "ancient", "last_reply_at": NOW - 400 * DAY},
            {"id": "loaded", "last_reply_at": NOW - 2 * DAY}
        ]
    }))
    .unwrap();
    store.add_thread(
        &"loaded".into(),
        &serde_json::from_value(json!({
            "order": ["r1", "loaded"],
            "posts": {
                "loaded": {"id": "loaded", "channel_id": "seen", "create_at": NOW - 5 * DAY},
                "r1": {"id": "r1", "channel_id": "seen", "root_id": "loaded",
                       "create_at": NOW - 2 * DAY}
            }
        }))
        .unwrap(),
    );
    let roots = |history| threads_to_load(&store, &list, history, NOW);
    assert_eq!(roots(History::Days(180)), [PostId::from("fresh")]);
    assert_eq!(
        roots(History::All),
        [PostId::from("fresh"), PostId::from("ancient")]
    );
    assert!(roots(History::Off).is_empty());
}

#[test]
fn the_queue_orders_by_priority_and_skips_duplicates() {
    let mut queue = Queue::default();
    assert!(queue.push(Priority::Backfill, Job::Newest("b".into())));
    assert!(queue.push(Priority::Recent, Job::Newest("a".into())));
    assert!(
        !queue.push(Priority::Recent, Job::Newest("a".into())),
        "duplicate"
    );
    assert!(queue.push(Priority::Interactive, Job::Thread("t".into())));
    assert_eq!(queue.len(), 3);

    let (priority, job) = queue.pop().unwrap();
    assert_eq!(
        (priority, job.clone()),
        (Priority::Interactive, Job::Thread("t".into()))
    );
    // A running job still counts as a duplicate until it finishes.
    assert!(!queue.push(Priority::Interactive, job.clone()));
    queue.finished(&job);
    assert!(queue.push(Priority::Interactive, job));

    // A retried job goes to the front of its class.
    queue.pop();
    let (_, next) = queue.pop().unwrap();
    assert_eq!(next, Job::Newest("a".into()));
    queue.push(Priority::Recent, Job::Newest("c".into()));
    queue.retry(Priority::Recent, next.clone());
    assert_eq!(queue.pop().unwrap().1, next);
    assert_eq!(queue.pop().unwrap().1, Job::Newest("c".into()));
    assert_eq!(queue.pop_interactive(), None);
}

#[test]
fn a_post_and_the_pages_around_it_make_one_page() {
    let post = |id: &str, at: i64| json!({"id": id, "channel_id": "c", "create_at": at});
    let list = |posts: &[(&str, i64)], prev: &str, next: &str| -> PostList {
        serde_json::from_value(json!({
            "order": posts.iter().rev().map(|(id, _)| *id).collect::<Vec<_>>(),
            "posts": posts.iter().map(|(id, at)| (id.to_string(), post(id, *at)))
                .collect::<serde_json::Map<_, _>>(),
            "prev_post_id": prev, "next_post_id": next,
        }))
        .unwrap()
    };
    let target: Post = serde_json::from_value(post("t", 30)).unwrap();
    let older = list(&[("a", 10), ("b", 20)], "z", "");
    let newer = list(&[("c", 40), ("d", 50)], "", "e");
    let around = around_list(target, older, newer);
    assert_eq!(around.order, ["d", "c", "t", "b", "a"].map(PostId::from));
    assert_eq!(around.posts.len(), 5);
    // Older posts exist before "a" and newer ones after "d".
    assert_eq!(around.prev_post_id, PostId::from("z"));
    assert_eq!(around.next_post_id, PostId::from("e"));
}
