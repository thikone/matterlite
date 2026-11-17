// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::ws::{EventFrame, WsEvent};
use mm_model::{Channel, ChannelMember, ClientConfig, PostList, User};
use serde_json::{Value, json};

use super::*;
use crate::store::PageRequest;

const ME: &str = "alice";

/// Alice in team `t1` with channels `c1` (10 posts, 8 roots, all read) and `c2`.
fn new_store(crt: &str) -> Store {
    let mut store = Store::new(User {
        id: ME.into(),
        username: "alice".into(),
        ..User::default()
    });
    store.set_config(
        serde_json::from_value::<ClientConfig>(json!({"CollapsedThreads": crt})).unwrap(),
    );
    let channel = |id: &str| Channel {
        id: id.into(),
        team_id: "t1".into(),
        name: id.into(),
        total_msg_count: 10,
        total_msg_count_root: 8,
        ..Channel::default()
    };
    let member = |id: &str| ChannelMember {
        channel_id: id.into(),
        user_id: ME.into(),
        msg_count: 10,
        msg_count_root: 8,
        ..ChannelMember::default()
    };
    store.set_teams(
        vec![mm_model::Team {
            id: "t1".into(),
            ..Default::default()
        }],
        vec![mm_model::TeamMember {
            team_id: "t1".into(),
            user_id: ME.into(),
            ..Default::default()
        }],
    );
    store.set_channels(
        vec![channel("c1"), channel("c2")],
        vec![member("c1"), member("c2")],
    );
    store.add_posts(
        &"c1".into(),
        &serde_json::from_value::<PostList>(json!({
            "order": ["root1"],
            "posts": {"root1": {"id": "root1", "channel_id": "c1", "user_id": "bob", "create_at": 100}},
            "next_post_id": "", "prev_post_id": ""
        }))
        .unwrap(),
        PageRequest::Newest,
    );
    store
}

fn event(name: &str, data: Value, broadcast: Value) -> (WsEvent, Broadcast) {
    let frame: EventFrame = serde_json::from_value(json!({
        "event": name, "data": data, "broadcast": broadcast, "seq": 1
    }))
    .unwrap();
    (WsEvent::decode(&frame).unwrap(), frame.broadcast)
}

fn posted(post: Value, mentions: &[&str]) -> (WsEvent, Broadcast) {
    let channel = post["channel_id"].as_str().unwrap().to_owned();
    event(
        "posted",
        json!({
            "post": post.to_string(),
            "team_id": "t1",
            "mentions": serde_json::to_string(mentions).unwrap(),
        }),
        json!({"channel_id": channel}),
    )
}

fn apply(store: &mut Store, (event, broadcast): (WsEvent, Broadcast)) -> Vec<StoreDelta> {
    store.apply(&event, &broadcast)
}

fn unread(store: &Store, channel: &str) -> (i64, i64) {
    let unread = store.channel_unread(&channel.into()).unwrap();
    (unread.messages, unread.mentions)
}

fn post(value: Value) -> Post {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_mention_from_someone_else_makes_the_channel_unread() {
    let mut store = new_store("always_on");
    let deltas = apply(
        &mut store,
        posted(
            json!({"id": "p1", "channel_id": "c1", "user_id": "bob", "create_at": 200,
                   "message": "@alice hi"}),
            &[ME],
        ),
    );
    assert_eq!(unread(&store, "c1"), (1, 1));
    assert_eq!(store.team_unread(&"t1".into()).mentions, 1);
    assert!(deltas.contains(&StoreDelta::TimelineChanged("c1".into())));
    assert_eq!(store.timelines[&ChannelId::from("c1")].len(), 2);
    assert_eq!(store.channels[&ChannelId::from("c1")].last_post_at, 200);
}

#[test]
fn own_posts_and_the_viewed_channel_stay_read() {
    let mut store = new_store("always_on");
    let deltas = apply(
        &mut store,
        posted(
            json!({"id": "p1", "channel_id": "c1", "user_id": ME, "create_at": 200}),
            &[],
        ),
    );
    assert_eq!(unread(&store, "c1"), (0, 0));
    assert!(deltas.contains(&StoreDelta::MarkedRead {
        channel: "c1".into(),
        notify_server: false
    }));

    store.viewing = Some("c2".into());
    let deltas = apply(
        &mut store,
        posted(
            json!({"id": "p2", "channel_id": "c2", "user_id": "bob", "create_at": 300}),
            &[ME],
        ),
    );
    assert_eq!(unread(&store, "c2"), (0, 0));
    assert!(deltas.contains(&StoreDelta::MarkedRead {
        channel: "c2".into(),
        notify_server: true
    }));
}

#[test]
fn muted_channels_and_uncounted_posts() {
    let mut store = new_store("always_on");
    store
        .channel_members
        .get_mut(&ChannelId::from("c2"))
        .unwrap()
        .notify_props
        .insert("mark_unread".into(), "mention".into());
    apply(
        &mut store,
        posted(
            json!({"id": "p1", "channel_id": "c2", "user_id": "bob", "create_at": 200}),
            &[],
        ),
    );
    assert_eq!(unread(&store, "c2"), (0, 0), "muted: stays read");

    // Join/leave messages aren't counted by the server, but "added me" is a mention.
    apply(
        &mut store,
        posted(
            json!({"id": "p2", "channel_id": "c1", "user_id": "sysadmin", "create_at": 300,
                   "type": "system_add_to_channel", "props": {"addedUserId": ME}}),
            &[ME],
        ),
    );
    assert_eq!(unread(&store, "c1"), (0, 1));
    assert_eq!(store.channels[&ChannelId::from("c1")].total_msg_count, 10);
}

#[test]
fn replies_under_crt_count_only_outside_the_root_counters() {
    let mut store = new_store("always_on");
    apply(
        &mut store,
        posted(
            json!({"id": "r1", "channel_id": "c1", "user_id": "bob", "create_at": 200,
                   "root_id": "root1"}),
            &[ME],
        ),
    );
    // The root counters (CRT) don't change; the plain ones do.
    assert_eq!(unread(&store, "c1"), (0, 0));
    let member = &store.channel_members[&ChannelId::from("c1")];
    assert_eq!((member.mention_count, member.mention_count_root), (1, 0));
    assert_eq!(store.channels[&ChannelId::from("c1")].total_msg_count, 11);
    // The reply isn't in the channel timeline; the root's counters moved.
    assert_eq!(store.timelines[&ChannelId::from("c1")].len(), 1);
    assert_eq!(store.posts[&PostId::from("root1")].reply_count, 1);
    assert_eq!(store.posts[&PostId::from("root1")].last_reply_at, 200);

    let mut flat = new_store("disabled");
    apply(
        &mut flat,
        posted(
            json!({"id": "r1", "channel_id": "c1", "user_id": "bob", "create_at": 200,
                   "root_id": "root1"}),
            &[],
        ),
    );
    assert_eq!(unread(&flat, "c1"), (1, 0));
    assert_eq!(flat.timelines[&ChannelId::from("c1")].len(), 2);
}

#[test]
fn pending_posts_are_replaced_once_whichever_reply_comes_first() {
    for confirm_first in [false, true] {
        let mut store = new_store("always_on");
        store.add_pending(post(json!({
            "channel_id": "c1", "message": "hello", "create_at": 500,
            "pending_post_id": "alice:500"
        })));
        assert!(store.timelines[&ChannelId::from("c1")].contains(&PostId::from("alice:500")));

        let server = json!({"id": "real1", "channel_id": "c1", "user_id": ME,
            "message": "hello", "create_at": 510, "pending_post_id": "alice:500"});
        let server_post = post(server.clone());
        if confirm_first {
            store.confirm_post(&server_post);
            apply(&mut store, posted(server, &[]));
        } else {
            apply(&mut store, posted(server, &[]));
            store.confirm_post(&server_post);
        }
        let timeline = &store.timelines[&ChannelId::from("c1")];
        assert_eq!(
            timeline.len(),
            2,
            "root1 and real1 (confirm first: {confirm_first})"
        );
        assert!(timeline.contains(&PostId::from("real1")));
        assert!(!store.posts.contains_key(&PostId::from("alice:500")));
        assert!(store.pending.is_empty());
        assert_eq!(unread(&store, "c1"), (0, 0));
        assert_eq!(store.channels[&ChannelId::from("c1")].total_msg_count, 11);
    }
}

#[test]
fn failed_pending_posts_can_be_discarded() {
    let mut store = new_store("always_on");
    store.add_pending(post(json!({
        "channel_id": "c1", "create_at": 500, "pending_post_id": "alice:500"
    })));
    store.fail_pending("alice:500");
    assert!(store.pending["alice:500"].failed);
    store.discard_pending("alice:500");
    assert!(store.pending.is_empty());
    assert_eq!(store.timelines[&ChannelId::from("c1")].len(), 1);
}

#[test]
fn reactions_and_deletes() {
    let mut store = new_store("always_on");
    let reaction = json!({"reaction": json!({"user_id": "bob", "post_id": "root1",
        "emoji_name": "+1"}).to_string()});
    apply(
        &mut store,
        event("reaction_added", reaction.clone(), json!({})),
    );
    apply(
        &mut store,
        event("reaction_added", reaction.clone(), json!({})),
    );
    let root = &store.posts[&PostId::from("root1")];
    assert_eq!(
        root.metadata.as_ref().unwrap().reactions.len(),
        1,
        "no duplicates"
    );
    assert!(root.has_reactions);
    apply(&mut store, event("reaction_removed", reaction, json!({})));
    assert!(!store.posts[&PostId::from("root1")].has_reactions);

    // A loaded thread, then the root is deleted: the replies go too.
    store.add_thread(
        &"root1".into(),
        &serde_json::from_value::<PostList>(json!({
            "order": ["r1", "root1"],
            "posts": {
                "root1": {"id": "root1", "channel_id": "c1", "create_at": 100},
                "r1": {"id": "r1", "channel_id": "c1", "root_id": "root1", "create_at": 150}
            }
        }))
        .unwrap(),
    );
    let deltas = apply(
        &mut store,
        event(
            "post_deleted",
            json!({"post": json!({"id": "root1", "channel_id": "c1", "delete_at": 9}).to_string()}),
            json!({"channel_id": "c1"}),
        ),
    );
    assert!(deltas.contains(&StoreDelta::PostsRemoved(vec!["root1".into(), "r1".into()])));
    assert!(store.posts.is_empty());
    assert!(store.timelines[&ChannelId::from("c1")].is_empty());
}

#[test]
fn read_state_from_other_devices() {
    let mut store = new_store("always_on");
    apply(
        &mut store,
        posted(
            json!({"id": "p1", "channel_id": "c1", "user_id": "bob", "create_at": 200}),
            &[ME],
        ),
    );
    apply(
        &mut store,
        event(
            "multiple_channels_viewed",
            json!({"channel_times": {"c1": 250}}),
            json!({}),
        ),
    );
    assert_eq!(unread(&store, "c1"), (0, 0));
    assert_eq!(
        store.channel_members[&ChannelId::from("c1")].last_viewed_at,
        250
    );

    // Marked unread by hand: a post while viewing doesn't clear it. The channel comes in the
    // broadcast only (`sendWebSocketPostUnreadEvent`).
    apply(
        &mut store,
        event(
            "post_unread",
            json!({"msg_count": 9, "msg_count_root": 7, "mention_count": 1,
                   "mention_count_root": 1, "urgent_mention_count": 0,
                   "last_viewed_at": 150, "post_id": "root1"}),
            json!({"channel_id": "c1", "team_id": "t1", "user_id": ME}),
        ),
    );
    assert_eq!(
        store.channel_members[&ChannelId::from("c1")].last_viewed_at,
        150
    );
    store.viewing = Some("c1".into());
    apply(
        &mut store,
        posted(
            json!({"id": "p2", "channel_id": "c1", "user_id": "bob", "create_at": 300}),
            &[],
        ),
    );
    assert_eq!(unread(&store, "c1"), (3, 1));
    store.mark_viewed(&"c1".into(), 400);
    assert_eq!(unread(&store, "c1"), (0, 0));
    assert!(store.manually_unread.is_empty());
}

#[test]
fn membership_and_unknown_channels() {
    let mut store = new_store("always_on");
    let deltas = apply(
        &mut store,
        posted(
            json!({"id": "d1", "channel_id": "dm1", "user_id": "bob", "create_at": 1}),
            &[ME],
        ),
    );
    assert!(deltas.contains(&StoreDelta::Fetch(Fetch::Channel("dm1".into()))));

    let deltas = apply(
        &mut store,
        event(
            "user_removed",
            json!({"user_id": ME, "remover_id": "admin"}),
            json!({"channel_id": "c2"}),
        ),
    );
    assert_eq!(deltas, [StoreDelta::ChannelRemoved("c2".into())]);
    assert!(!store.channels.contains_key(&ChannelId::from("c2")));

    let deltas = apply(
        &mut store,
        event(
            "leave_team",
            json!({"team_id": "t1", "user_id": ME}),
            json!({}),
        ),
    );
    assert!(deltas.contains(&StoreDelta::ChannelRemoved("c1".into())));
    assert!(store.channels.is_empty());
}

#[test]
fn preferences_categories_and_threads() {
    let mut store = new_store("default_on");
    assert!(store.crt_enabled());
    let deltas = apply(
        &mut store,
        event(
            "preferences_changed",
            json!({"preferences": json!([{"user_id": ME, "category": "display_settings",
                "name": "collapsed_reply_threads", "value": "off"}]).to_string()}),
            json!({"user_id": ME}),
        ),
    );
    assert!(!store.crt_enabled());
    assert!(deltas.contains(&StoreDelta::CrtChanged));

    // The bare category update after a preference change does nothing.
    let bare = apply(
        &mut store,
        event(
            "sidebar_category_updated",
            json!({}),
            json!({"user_id": ME}),
        ),
    );
    assert!(bare.is_empty());
    let with_team = apply(
        &mut store,
        event(
            "sidebar_category_updated",
            json!({}),
            json!({"team_id": "t1"}),
        ),
    );
    assert_eq!(
        with_team,
        [StoreDelta::Fetch(Fetch::Categories("t1".into()))]
    );

    apply(
        &mut store,
        event(
            "thread_updated",
            json!({"thread": json!({"id": "root1", "reply_count": 2, "unread_replies": 2,
                "unread_mentions": 1}).to_string(),
                "previous_unread_replies": 0, "previous_unread_mentions": 0}),
            json!({"team_id": "t1"}),
        ),
    );
    let counts = store.thread_counts[&TeamId::from("t1")];
    assert_eq!((counts.unread_threads, counts.mentions), (1, 1));
    apply(
        &mut store,
        event(
            "thread_read_changed",
            json!({"thread_id": "root1", "timestamp": 5, "unread_replies": 0, "unread_mentions": 0,
                   "previous_unread_replies": 2, "previous_unread_mentions": 1}),
            json!({"team_id": "t1"}),
        ),
    );
    let counts = store.thread_counts[&TeamId::from("t1")];
    assert_eq!((counts.unread_threads, counts.mentions), (0, 0));
}
