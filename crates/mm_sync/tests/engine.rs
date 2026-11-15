// SPDX-License-Identifier: AGPL-3.0-only

//! The engine against a mock server that answers with the responses captured from the test
//! server (`mm_model/fixtures/captured`). The WebSocket upgrade fails against the mock, so
//! these runs are "online over REST, offline over WebSocket".

use std::path::Path;
use std::time::Duration;

use mm_api::ServerUrl;
use mm_cache::Cache;
use mm_model::{ChannelId, PostId, SessionToken};
use mm_sync::{Command, EngineConfig, EngineHandle, Event};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ALICE: &str = "y5uhpy7zmpdi9q5kgnuwaa34sy";
const WAIT: Duration = Duration::from_secs(15);

macro_rules! captured {
    ($name:literal) => {
        include_str!(concat!("../../mm_model/fixtures/captured/", $name))
    };
}

fn json_body(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(text.to_owned(), "application/json")
}

fn dev_channel() -> String {
    let channels: Vec<Value> = serde_json::from_str(captured!("channels.json")).unwrap();
    channels
        .iter()
        .find(|channel| channel["name"] == "dev")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn server() -> MockServer {
    let server = MockServer::start().await;
    let get = |route: &str, body: &str| {
        Mock::given(method("GET"))
            .and(path(format!("/api/v4{route}")))
            .respond_with(json_body(body))
    };
    for (route, body) in [
        ("/config/client", captured!("config_client.json")),
        ("/license/client", captured!("license_client.json")),
        ("/users/me", captured!("user_me.json")),
        ("/users/me/preferences", captured!("preferences.json")),
        ("/users/me/teams", captured!("teams.json")),
        ("/users/me/teams/members", captured!("team_members.json")),
        ("/users/me/teams/unread", captured!("team_unreads.json")),
        ("/users/me/channels", captured!("channels.json")),
    ] {
        get(route, body).mount(&server).await;
    }
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/users/{ALICE}/channel_members")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(captured!("channel_members.ndjson"), "application/x-ndjson"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/v4/users/[a-z0-9]+/teams/[a-z0-9]+/channels/categories$",
        ))
        .respond_with(json_body(captured!("sidebar_categories.json")))
        .mount(&server)
        .await;
    for (route, body) in [
        ("/roles/names", captured!("roles.json")),
        ("/users/ids", captured!("users.json")),
        ("/users/status/ids", captured!("statuses.json")),
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/api/v4{route}")))
            .respond_with(json_body(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("PUT"))
        .and(path_regex(r"^/api/v4/users/[a-z0-9]+/preferences$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let dev = dev_channel();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v4/users/{ALICE}/channels/{dev}/posts/unread"
        )))
        .respond_with(json_body(captured!("dev_posts.json")))
        .mount(&server)
        .await;
    // `#dev` answers every page request with the captured page; other channels are empty.
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/{dev}/posts")))
        .respond_with(json_body(captured!("dev_posts.json")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v4/channels/[a-z0-9]+/posts$"))
        .respond_with(json_body(
            r#"{"order": [], "posts": {}, "next_post_id": "", "prev_post_id": ""}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/members/me/view"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/posts"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            ResponseTemplate::new(201).set_body_json(json!({
                "id": "realpost00000000000000000a",
                "channel_id": body["channel_id"],
                "user_id": ALICE,
                "message": body["message"],
                "pending_post_id": body["pending_post_id"],
                "create_at": 1_900_000_000_000_i64,
                "update_at": 1_900_000_000_000_i64,
            }))
        })
        .mount(&server)
        .await;
    server
}

fn start(server: &MockServer, dir: &Path, known_user: bool) -> EngineHandle {
    let mut config = EngineConfig::new(
        ServerUrl::parse(&server.uri()).unwrap(),
        SessionToken::new("token"),
        dir.to_owned(),
    );
    if known_user {
        config.user_id = Some(ALICE.into());
    }
    mm_sync::start(config)
}

/// Events until one matches; all of them, in order.
async fn until(engine: &EngineHandle, matches: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, engine.events().recv())
            .await
            .unwrap_or_else(|_| panic!("timed out; events so far: {seen:#?}"))
            .unwrap();
        let done = matches(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

/// Waits for changes until the store satisfies `done`.
async fn until_store(engine: &EngineHandle, done: impl Fn(&mm_store::Store) -> bool) {
    loop {
        if done(&engine.store().read().unwrap()) {
            return;
        }
        tokio::time::timeout(WAIT, engine.events().recv())
            .await
            .expect("timed out waiting for the store")
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn first_start_bootstraps_and_fills_the_cache() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&server, dir.path(), false);
    let events = until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    assert!(!events.contains(&Event::Hydrated), "nothing cached yet");
    assert!(events.contains(&Event::Ready { resumed: false }));
    {
        let store = engine.store().read().unwrap();
        assert_eq!(store.me.as_str(), ALICE);
        assert_eq!(store.teams.len(), 2);
        assert!(store.crt_enabled());
    }
    // The unread DMs got their show preference (`direct_channel_show`).
    let saved: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.method == wiremock::http::Method::PUT)
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(saved.len(), 1);
    assert!(
        saved[0]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["category"] == "direct_channel_show")
    );
    engine.shutdown();

    // Everything is in the cache now.
    let cache_dir = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let (mut cache, _) = Cache::open(&cache_dir.join("cache.sqlite")).unwrap();
    let hydrated = cache.hydrate().unwrap().unwrap();
    assert_eq!(hydrated.teams.len(), 2);
    assert_eq!(hydrated.channels.len(), 16);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_known_account_draws_from_the_cache_first() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    start(&server, dir.path(), false).shutdown();

    let engine = start(&server, dir.path(), true);
    let events = until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    assert!(matches!(events[0], Event::CacheOpened(_)), "{events:#?}");
    assert_eq!(events[1], Event::Hydrated, "{events:#?}");
    // The WebSocket is down (the mock has none), so the saved session can't resume.
    assert!(events.contains(&Event::Ready { resumed: false }));
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_a_channel_loads_the_unread_window_and_marks_it_read() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&server, dir.path(), false);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    let dev = ChannelId::new(dev_channel());
    assert!(
        engine
            .store()
            .read()
            .unwrap()
            .channel_unread(&dev)
            .unwrap()
            .is_unread()
    );

    engine.send(Command::OpenChannel(dev.clone()));
    let events = until(&engine, |event| {
        matches!(event, Event::ChannelOpened { .. })
    })
    .await;
    assert!(events.iter().any(
        |event| matches!(event, Event::ChannelOpened { channel, last_viewed_at }
                if *channel == dev && *last_viewed_at > 0)
    ));
    // The page arrives from a job, after the read mark.
    until_store(&engine, |store| {
        store.timeline_posts(&dev).iter().flatten().count() == 16
    })
    .await;
    {
        let store = engine.store().read().unwrap();
        assert_eq!(store.timeline_posts(&dev).iter().flatten().count(), 16);
        assert!(!store.channel_unread(&dev).unwrap().is_unread());
    }
    // The server is told too (in the background).
    let mut viewed = false;
    for _ in 0..50 {
        viewed = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == "/api/v4/channels/members/me/view");
        if viewed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(viewed);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sent_post_replaces_its_pending_copy() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&server, dir.path(), false);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    let dev = ChannelId::new(dev_channel());
    engine.send(Command::OpenChannel(dev.clone()));
    until(&engine, |event| {
        matches!(event, Event::ChannelOpened { .. })
    })
    .await;

    engine.send(Command::SendPost {
        channel: dev.clone(),
        root: PostId::default(),
        message: "hello from the engine".into(),
    });
    until(&engine, |event| {
        matches!(event, Event::Changed(deltas) if deltas.iter().any(|d|
            matches!(d, mm_store::StoreDelta::PostsRemoved(ids)
                if ids.iter().any(|id| id.as_str().starts_with(ALICE)))))
    })
    .await;
    {
        let store = engine.store().read().unwrap();
        assert!(store.pending.is_empty());
        // The captured page doesn't reach the newest post, so the timeline shows it below a
        // hole and the new post waits there; the store has the server's copy.
        let sent = &store.posts[&PostId::new("realpost00000000000000000a")];
        assert_eq!(sent.message, "hello from the engine");
        assert!(!store.posts.keys().any(|id| id.as_str().starts_with(ALICE)));
    }
    let sent = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path() == "/api/v4/posts")
        .unwrap();
    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    assert!(
        body["pending_post_id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{ALICE}:"))
    );
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_token_logs_out_and_keeps_the_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "api.context.session_expired.app_error",
            "message": "Invalid or expired session, please login again.",
            "status_code": 401
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&server, dir.path(), true);
    let events = until(&engine, |event| *event == Event::LoggedOut).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Ready { .. }))
    );
    until(&engine, |event| *event == Event::Stopped).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_429_pauses_the_sync_and_the_job_is_retried() {
    let server = server().await;
    let dev = dev_channel();
    // The first request for #dev's posts is rate limited.
    Mock::given(method("GET"))
        .and(path(format!("/api/v4/channels/{dev}/posts")))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "1")
                .set_body_string("rate limit exceeded"),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let engine = start(&server, dir.path(), false);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    let ready = std::time::Instant::now();
    let dev_id = ChannelId::new(dev.clone());
    until_store(&engine, |store| {
        store
            .timelines
            .get(&dev_id)
            .is_some_and(|timeline| !timeline.is_empty())
    })
    .await;
    assert!(
        ready.elapsed() >= Duration::from_millis(900),
        "waited {:?}",
        ready.elapsed()
    );
    let dev_requests = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == format!("/api/v4/channels/{dev}/posts"))
        .count();
    assert_eq!(dev_requests, 2, "the 429 and the retry");
    engine.shutdown();
}

/// The cache file of alice on this mock server.
fn account_dir(server: &MockServer, servers: &Path) -> std::path::PathBuf {
    let url = ServerUrl::parse(&server.uri()).unwrap();
    servers.join(mm_cache::account_id(url.as_str(), &ALICE.into()))
}

fn outbox_entry(pending: &str, channel: &str, message: &str, at: i64) -> mm_cache::OutboxEntry {
    mm_cache::OutboxEntry {
        pending_post_id: pending.into(),
        channel_id: channel.into(),
        root_id: PostId::default(),
        message: message.into(),
        props: Default::default(),
        file_ids: Vec::new(),
        first_attempt_at: at,
        last_attempt_at: at,
        failed: false,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn town_square() -> String {
    let channels: Vec<Value> = serde_json::from_str(captured!("channels.json")).unwrap();
    channels
        .iter()
        .find(|channel| channel["name"] == "town-square")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_outbox_is_resent_or_reconciled_after_a_restart() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    start(&server, dir.path(), false).shutdown();

    // Three posts were being sent when the app stopped:
    let dev = dev_channel();
    let showcase: Vec<Value> = serde_json::from_str::<Value>(captured!("dev_posts.json")).unwrap()
        ["posts"]
        .as_object()
        .unwrap()
        .values()
        .filter(|post| post["message"].as_str().unwrap().starts_with("# Heading 1"))
        .cloned()
        .collect();
    let showcase_message = showcase[0]["message"].as_str().unwrap().to_owned();
    let entries = vec![
        // tried a moment ago: sent again with the same pending id;
        outbox_entry(&format!("{ALICE}:1"), &dev, "just now", now_ms() - 5_000),
        // tried long ago and on the server (the showcase post is by alice): confirmed;
        outbox_entry(
            &format!("{ALICE}:2"),
            &dev,
            &showcase_message,
            now_ms() - 3_600_000,
        ),
        // tried long ago and not on the server: failed, not re-sent.
        outbox_entry(
            &format!("{ALICE}:3"),
            &town_square(),
            "lost",
            now_ms() - 3_600_000,
        ),
    ];
    {
        let path = account_dir(&server, dir.path()).join("cache.sqlite");
        let (mut cache, _) = Cache::open(&path).unwrap();
        let ops: Vec<mm_cache::Op> = entries.into_iter().map(mm_cache::Op::PutOutbox).collect();
        cache.apply(&ops).unwrap();
    }

    let engine = start(&server, dir.path(), true);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    until_store(&engine, |store| {
        store.pending.len() == 1 && store.pending.values().all(|pending| pending.failed)
    })
    .await;
    {
        let store = engine.store().read().unwrap();
        assert!(store.pending.contains_key(&format!("{ALICE}:3")));
    }
    let sent: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/api/v4/posts")
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert_eq!(sent[0]["pending_post_id"], format!("{ALICE}:1"));
    engine.shutdown();

    // Only the failed one is left in the outbox.
    let path = account_dir(&server, dir.path()).join("cache.sqlite");
    let (mut cache, _) = Cache::open(&path).unwrap();
    let outbox = cache.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].pending_post_id, format!("{ALICE}:3"));
    assert!(outbox[0].failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reset_server_asks_and_archives_the_old_cache() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    start(&server, dir.path(), false).shutdown();
    let account = account_dir(&server, dir.path());
    {
        // The cache remembers a different server identity.
        let (mut cache, _) = Cache::open(&account.join("cache.sqlite")).unwrap();
        cache
            .apply(&[mm_cache::Op::Kv(
                mm_cache::kv::DIAGNOSTIC_ID.into(),
                Some("another-server".into()),
            )])
            .unwrap();
    }

    let engine = start(&server, dir.path(), true);
    let events = until(&engine, |event| *event == Event::ServerChanged).await;
    assert!(matches!(events[0], Event::CacheOpened(_)));
    assert_eq!(events[1], Event::Hydrated);
    engine.send(Command::ResolveServerChange(
        mm_sync::ServerChange::ArchiveAndStartFresh,
    ));
    until(&engine, |event| {
        matches!(event, Event::Ready { resumed: false })
    })
    .await;
    engine.shutdown();

    let archives: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".archived-"))
        .collect();
    assert_eq!(archives.len(), 1);
    // The new cache carries the new identity.
    let (mut cache, _) = Cache::open(&account.join("cache.sqlite")).unwrap();
    assert_eq!(
        cache
            .get_kv(mm_cache::kv::DIAGNOSTIC_ID)
            .unwrap()
            .as_deref(),
        Some("diagnosticidplaceholder000")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_keeps_the_data_unless_asked() {
    let server = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/logout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let account = account_dir(&server, dir.path());

    let engine = start(&server, dir.path(), false);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    engine.send(Command::Logout { delete_data: false });
    until(&engine, |event| *event == Event::Stopped).await;
    assert!(account.join("cache.sqlite").exists());
    assert_eq!(mm_cache::stored_accounts(dir.path()).len(), 1);

    let engine = start(&server, dir.path(), true);
    until(&engine, |event| matches!(event, Event::Ready { .. })).await;
    engine.send(Command::Logout { delete_data: true });
    until(&engine, |event| *event == Event::Stopped).await;
    assert!(!account.exists());
    let logouts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == "/api/v4/users/logout")
        .count();
    assert_eq!(logouts, 2);
}
