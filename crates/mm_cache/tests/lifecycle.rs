// SPDX-License-Identifier: AGPL-3.0-only

//! Outbox, former chats, stored accounts and batch atomicity.

use mm_cache::{Cache, Op, OutboxEntry, snapshot_ops};
use mm_model::{Channel, ChannelId, ChannelMember, Post, PostId, User};
use mm_store::{PageRequest, Store};
use serde_json::json;

fn store() -> Store {
    let mut store = Store::new(User {
        id: "me".into(),
        username: "me".into(),
        ..User::default()
    });
    let channel = |id: &str| Channel {
        id: id.into(),
        team_id: "t".into(),
        name: id.into(),
        ..Channel::default()
    };
    let member = |id: &str| ChannelMember {
        channel_id: id.into(),
        user_id: "me".into(),
        ..ChannelMember::default()
    };
    store.set_channels(
        vec![channel("a"), channel("b")],
        vec![member("a"), member("b")],
    );
    for channel in ["a", "b"] {
        let id = format!("{channel}1");
        store.add_posts(
            &channel.into(),
            &serde_json::from_value(json!({
                "order": [id],
                "posts": {id.clone(): {"id": id, "channel_id": channel,
                          "message": format!("hello from {channel}"), "create_at": 5}},
                "next_post_id": "", "prev_post_id": ""
            }))
            .unwrap(),
            PageRequest::Newest,
        );
    }
    store
}

fn entry(pending: &str, failed: bool) -> OutboxEntry {
    OutboxEntry {
        pending_post_id: pending.into(),
        channel_id: "a".into(),
        root_id: Default::default(),
        message: "unsent".into(),
        props: serde_json::from_value(json!({"from_cli": true})).unwrap(),
        file_ids: vec!["f1".into()],
        first_attempt_at: 100,
        last_attempt_at: 120,
        failed,
    }
}

#[test]
fn the_outbox_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    {
        let (mut cache, _) = Cache::open(&path).unwrap();
        cache
            .apply(&[
                Op::PutOutbox(entry("me:100", false)),
                Op::PutOutbox(OutboxEntry {
                    first_attempt_at: 200,
                    ..entry("me:200", true)
                }),
            ])
            .unwrap();
    }
    let (mut cache, _) = Cache::open(&path).unwrap();
    let outbox = cache.outbox().unwrap();
    assert_eq!(outbox.len(), 2);
    assert_eq!(outbox[0], entry("me:100", false));
    assert!(outbox[1].failed);
    cache.apply(&[Op::DeleteOutbox("me:100".into())]).unwrap();
    assert_eq!(cache.outbox().unwrap().len(), 1);
}

#[test]
fn former_chats_are_kept_revived_and_purged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let (mut cache, _) = Cache::open(&path).unwrap();
    let store = store();
    cache.apply(&snapshot_ops(&store)).unwrap();
    assert!(cache.former_chats().unwrap().is_empty());

    // A new channel list without `b`: it becomes a former chat, its posts stay.
    cache
        .apply(&[Op::KeepChannelMembers(vec!["a".into()])])
        .unwrap();
    let former = cache.former_chats().unwrap();
    assert_eq!(former.len(), 1);
    assert_eq!(former[0].channel.id.as_str(), "b");
    assert_eq!(former[0].posts, 1);
    let mut reader = mm_cache::open_reader(&path).unwrap();
    let posts = mm_cache::former_posts(&mut reader, &"b".into(), 10).unwrap();
    assert_eq!(posts[0].message, "hello from b");
    // Hydration leaves it out.
    let hydrated = cache.hydrate().unwrap().unwrap();
    assert!(!hydrated.channels.contains_key(&ChannelId::from("b")));

    // Rejoining revives it.
    let member = store.channel_members[&ChannelId::from("b")].clone();
    cache.apply(&[Op::PutChannelMember(member)]).unwrap();
    assert!(cache.former_chats().unwrap().is_empty());

    // Removed again, then purged after the retention: nothing of it is left.
    cache.apply(&[Op::DeleteChannelMember("b".into())]).unwrap();
    assert_eq!(
        cache.expired_former_chats(i64::MAX).unwrap(),
        [ChannelId::from("b")]
    );
    assert!(cache.expired_former_chats(0).unwrap().is_empty());
    cache.apply(&[Op::PurgeChannel("b".into())]).unwrap();
    assert!(cache.former_chats().unwrap().is_empty());
    let hits = cache.search("hello", None, 10).unwrap();
    assert_eq!(
        hits.len(),
        1,
        "only the post of channel a is left in the index"
    );
    assert_eq!(hits[0].channel.as_str(), "a");
}

#[test]
fn a_batch_is_all_or_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite");
    let (mut cache, _) = Cache::open(&path).unwrap();
    cache.apply(&snapshot_ops(&store())).unwrap();
    let post: Post = serde_json::from_value(json!({
        "id": "a2", "channel_id": "a", "message": "new", "create_at": 9
    }))
    .unwrap();
    // A post and its timeline cursor in one batch, where the cursor write fails (the same
    // range twice violates the primary key): neither may be written.
    let range = mm_store::Range {
        oldest: (1, "a1".into()),
        newest: (9, "a2".into()),
        recent: true,
        start: true,
    };
    let result = cache.apply(&[
        Op::PutPost(post),
        Op::SetRanges {
            owner: mm_cache::Owner::Channel,
            id: "a".into(),
            ranges: vec![range.clone(), range],
            empty: false,
        },
    ]);
    assert!(result.is_err());
    let hydrated = cache.hydrate().unwrap().unwrap();
    assert!(!hydrated.posts.contains_key(&PostId::from("a2")));
    assert_eq!(
        hydrated.timelines[&ChannelId::from("a")].ranges()[0]
            .newest
            .1
            .as_str(),
        "a1",
        "the old cursor stays"
    );
}

#[test]
fn stored_accounts_are_listed_and_deleted() {
    let servers = tempfile::tempdir().unwrap();
    let account = servers.path().join(mm_cache::account_id(
        "https://chat.example.com",
        &"me".into(),
    ));
    {
        let (mut cache, _) = Cache::open(&account.join("cache.sqlite")).unwrap();
        let mut ops = snapshot_ops(&store());
        ops.push(Op::Kv(
            mm_cache::kv::SERVER_URL.into(),
            Some("https://chat.example.com".into()),
        ));
        ops.push(Op::Kv(mm_cache::kv::LAST_USED.into(), Some("1000".into())));
        ops.push(Op::PutOutbox(entry("me:1", true)));
        cache.apply(&ops).unwrap();
    }
    // A directory without a cache is not an account.
    std::fs::create_dir_all(servers.path().join("junk")).unwrap();
    let accounts = mm_cache::stored_accounts(servers.path());
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].user.username, "me");
    assert_eq!(
        accounts[0].server_url.as_deref(),
        Some("https://chat.example.com")
    );
    assert_eq!(accounts[0].last_used, Some(1000));
    assert_eq!(accounts[0].unsent, 1);
    assert!(accounts[0].size > 0);

    const DAY: i64 = 86_400_000;
    assert!(mm_cache::delete_unused_accounts(servers.path(), 30, 1000 + 29 * DAY).is_empty());
    let deleted = mm_cache::delete_unused_accounts(servers.path(), 30, 1000 + 31 * DAY);
    assert_eq!(deleted, std::slice::from_ref(&account));
    assert!(!account.exists());
    assert!(mm_cache::stored_accounts(servers.path()).is_empty());
}

#[test]
fn ui_state_is_read_back_without_its_prefix() {
    let dir = tempfile::tempdir().unwrap();
    assert!(mm_cache::ui_state(dir.path()).is_empty(), "no cache yet");
    let (mut cache, _) = Cache::open(&dir.path().join("cache.sqlite")).unwrap();
    cache.apply(&snapshot_ops(&store())).unwrap();
    cache
        .apply(&[
            Op::Kv("ui.last_team".into(), Some("t".into())),
            Op::Kv("ui.collapsed".into(), Some("[]".into())),
            Op::Kv("ui.gone".into(), None),
        ])
        .unwrap();
    drop(cache);
    let state = mm_cache::ui_state(dir.path());
    assert_eq!(state.len(), 2, "{state:?}");
    assert_eq!(state["last_team"], "t");
}
