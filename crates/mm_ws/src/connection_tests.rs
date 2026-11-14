// SPDX-License-Identifier: AGPL-3.0-only

//! The connection task against a scripted mock server.

// tungstenite's handshake callbacks return its large `ErrorResponse` by value.
#![allow(clippy::result_large_err)]

use std::net::SocketAddr;

use mm_model::ws::Presence;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

struct MockConnection {
    ws: WebSocketStream<TcpStream>,
    uri: String,
    authorization: Option<String>,
}

async fn listen() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

async fn accept(listener: &TcpListener) -> MockConnection {
    let (stream, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
    let mut uri = String::new();
    let mut authorization = None;
    let ws = accept_hdr_async(stream, |request: &Request, response: Response| {
        uri = request.uri().to_string();
        authorization = request
            .headers()
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_owned());
        Ok(response)
    })
    .await
    .unwrap();
    MockConnection {
        ws,
        uri,
        authorization,
    }
}

impl MockConnection {
    async fn recv(&mut self) -> Value {
        loop {
            match timeout(WAIT, self.ws.next()).await.unwrap() {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str()).unwrap();
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    async fn send(&mut self, value: Value) {
        self.ws
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

    /// Reads the authentication challenge and the first ping and answers the ping.
    async fn handshake(&mut self) {
        let challenge = self.recv().await;
        assert_eq!(challenge["action"], "authentication_challenge");
        assert_eq!(challenge["seq"], 1);
        assert_eq!(challenge["data"]["token"], "tok");
        let ping = self.recv().await;
        assert_eq!(ping["action"], "ping");
        self.send(json!({"status": "OK", "seq_reply": ping["seq"], "data": {"text": "pong"}}))
            .await;
    }

    async fn event(&mut self, event: &str, seq: i64, data: Value) {
        self.send(json!({
            "event": event,
            "data": data,
            "broadcast": {"channel_id": "c1", "user_id": "", "team_id": "", "omit_users": null},
            "seq": seq,
        }))
        .await;
    }

    async fn hello(&mut self, connection_id: &str) {
        self.event(
            "hello",
            0,
            json!({"connection_id": connection_id, "server_version": "10.11.0", "server_hostname": "mm"}),
        )
        .await;
    }

    /// The close code the client sent.
    async fn expect_close(&mut self) -> Option<u16> {
        loop {
            match timeout(WAIT, self.ws.next()).await.unwrap() {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
                Some(Ok(Message::Text(_) | Message::Ping(_) | Message::Pong(_))) => continue,
                other => panic!("expected a close frame, got {other:?}"),
            }
        }
    }
}

fn config(addr: SocketAddr) -> WsConfig {
    WsConfig {
        backoff: Backoff {
            min: Duration::from_millis(10),
            max: Duration::from_millis(50),
            fails_before_growth: 7,
            jitter: Duration::ZERO,
        },
        ..WsConfig::new(
            Url::parse(&format!("ws://{addr}/api/v4/websocket")).unwrap(),
            SessionToken::new("tok"),
            "MatterLite-test".to_owned(),
        )
    }
}

async fn next(output: &Receiver<WsOutput>) -> WsOutput {
    timeout(WAIT, output.recv()).await.unwrap().unwrap()
}

fn event_name(output: &WsOutput) -> String {
    match output {
        WsOutput::Event { event, .. } => format!("{event:?}")
            .split(['(', ' '])
            .next()
            .unwrap()
            .to_owned(),
        other => panic!("expected an event, got {other:?}"),
    }
}

#[tokio::test]
async fn connects_authenticates_and_delivers_events() {
    let (listener, addr) = listen().await;
    let (handle, output) = connect(config(addr));

    let mut server = accept(&listener).await;
    assert_eq!(
        server.uri,
        "/api/v4/websocket?connection_id=&sequence_number=0"
    );
    assert_eq!(server.authorization.as_deref(), Some("Bearer tok"));
    server.handshake().await;
    server.hello("conn1").await;
    let post = json!({"id": "p1", "channel_id": "c1", "message": "hi"});
    server
        .event(
            "posted",
            1,
            json!({"post": post.to_string(), "mentions": "[]"}),
        )
        .await;
    server.event("brand_new_event", 2, json!({})).await;
    server.event("post_edited", 3, json!({})).await;

    assert!(matches!(
        next(&output).await,
        WsOutput::Connected { resuming: false }
    ));
    assert_eq!(event_name(&next(&output).await), "Hello");
    match next(&output).await {
        WsOutput::Event {
            seq: 1,
            broadcast,
            event: WsEvent::Posted(posted),
        } => {
            assert_eq!(posted.post.message, "hi");
            assert_eq!(broadcast.channel_id.as_str(), "c1");
        }
        other => panic!("expected posted, got {other:?}"),
    }
    assert!(matches!(
        next(&output).await,
        WsOutput::Event { event: WsEvent::Unknown(name), .. } if name == "brand_new_event"
    ));
    assert!(matches!(
        next(&output).await,
        WsOutput::Undecodable { seq: 3, .. }
    ));

    assert!(handle.send(ClientAction::Presence(Presence::Channel("c1".into()))));
    let request = server.recv().await;
    assert_eq!(request["action"], "presence");
    assert_eq!(
        request["seq"], 3,
        "after the challenge (1) and the ping (2)"
    );
    server.send(json!({"status": "OK", "seq_reply": 3})).await;
    assert!(matches!(
        next(&output).await,
        WsOutput::Response(WsResponse { seq_reply: 3, .. })
    ));

    handle.close();
    assert_eq!(server.expect_close().await, Some(NORMAL_CLOSE_CODE));
    assert!(matches!(next(&output).await, WsOutput::Closed));
    assert!(timeout(WAIT, output.recv()).await.unwrap().is_err());
}

#[tokio::test]
async fn a_gap_reconnects_and_the_server_replays() {
    let (listener, addr) = listen().await;
    let (_handle, output) = connect(config(addr));

    let mut first = accept(&listener).await;
    first.handshake().await;
    first.hello("conn1").await;
    first.event("typing", 1, json!({"user_id": "u1"})).await;
    first.event("typing", 3, json!({"user_id": "u1"})).await;
    assert_eq!(first.expect_close().await, Some(SEQUENCE_GAP_CODE));

    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert_eq!(event_name(&next(&output).await), "Hello");
    assert_eq!(event_name(&next(&output).await), "Typing");
    assert!(matches!(
        next(&output).await,
        WsOutput::Disconnected {
            reason: DisconnectReason::SequenceGap {
                expected: 2,
                got: 3
            },
            ..
        }
    ));

    let mut second = accept(&listener).await;
    assert_eq!(
        second.uri,
        "/api/v4/websocket?connection_id=conn1&sequence_number=2&disconnect_err_code=4001"
    );
    second.handshake().await;
    // A lossless resume: no hello, the old sequence continues.
    second.event("typing", 2, json!({"user_id": "u2"})).await;
    second.event("typing", 3, json!({"user_id": "u1"})).await;
    assert!(matches!(
        next(&output).await,
        WsOutput::Connected { resuming: true }
    ));
    assert!(matches!(
        next(&output).await,
        WsOutput::Event { seq: 2, .. }
    ));
    assert!(matches!(
        next(&output).await,
        WsOutput::Event { seq: 3, .. }
    ));
}

#[tokio::test]
async fn a_new_connection_id_reports_missed_events() {
    let (listener, addr) = listen().await;
    let (_handle, output) = connect(config(addr));

    let mut first = accept(&listener).await;
    first.handshake().await;
    first.hello("conn1").await;
    first.ws.close(None).await.unwrap();

    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert_eq!(event_name(&next(&output).await), "Hello");
    assert!(matches!(
        next(&output).await,
        WsOutput::Disconnected {
            reason: DisconnectReason::ServerClosed { code: None, .. },
            ..
        }
    ));

    let mut second = accept(&listener).await;
    assert_eq!(
        second.uri,
        "/api/v4/websocket?connection_id=conn1&sequence_number=1"
    );
    second.handshake().await;
    second.hello("conn2").await;
    second.event("typing", 1, json!({"user_id": "u1"})).await;
    assert!(matches!(
        next(&output).await,
        WsOutput::Connected { resuming: true }
    ));
    assert!(matches!(next(&output).await, WsOutput::MissedEvents));
    assert_eq!(event_name(&next(&output).await), "Hello");
    assert!(matches!(
        next(&output).await,
        WsOutput::Event { seq: 1, .. }
    ));
}

#[tokio::test]
async fn an_unanswered_ping_reconnects() {
    let (listener, addr) = listen().await;
    let config = WsConfig {
        ping_interval: Duration::from_millis(100),
        ..config(addr)
    };
    let (_handle, output) = connect(config);

    let mut first = accept(&listener).await;
    assert_eq!(first.recv().await["action"], "authentication_challenge");
    assert_eq!(first.recv().await["action"], "ping");
    assert_eq!(first.expect_close().await, Some(PING_TIMEOUT_CODE));
    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert!(matches!(
        next(&output).await,
        WsOutput::Disconnected {
            reason: DisconnectReason::PingTimeout,
            ..
        }
    ));

    let second = accept(&listener).await;
    assert!(
        second.uri.ends_with("&disconnect_err_code=4000"),
        "{}",
        second.uri
    );
}

#[tokio::test]
async fn answered_pings_keep_the_connection() {
    let (listener, addr) = listen().await;
    let config = WsConfig {
        ping_interval: Duration::from_millis(50),
        ..config(addr)
    };
    let (handle, output) = connect(config);

    let mut server = accept(&listener).await;
    server.handshake().await;
    for _ in 0..3 {
        let ping = server.recv().await;
        assert_eq!(ping["action"], "ping");
        server
            .send(json!({"status": "OK", "seq_reply": ping["seq"], "data": {"text": "pong"}}))
            .await;
    }
    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert!(output.is_empty(), "pongs are not reported");
    handle.close();
    assert!(matches!(next(&output).await, WsOutput::Closed));
}

#[tokio::test]
async fn a_rejected_token_ends_the_task() {
    let (listener, addr) = listen().await;
    let (_handle, output) = connect(config(addr));

    let (stream, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
    let rejected = accept_hdr_async(stream, |_: &Request, _: Response| {
        Err::<Response, ErrorResponse>(
            http::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(None)
                .unwrap(),
        )
    })
    .await;
    assert!(rejected.is_err());

    assert!(matches!(next(&output).await, WsOutput::AuthFailed));
    assert!(timeout(WAIT, output.recv()).await.unwrap().is_err());
}

#[tokio::test]
async fn a_revoked_session_ends_the_task() {
    // Seen on a live server: after a revoke the socket stays open, events stop and every
    // request is answered with `not_authenticated`.
    let (listener, addr) = listen().await;
    let config = WsConfig {
        ping_interval: Duration::from_millis(50),
        ..config(addr)
    };
    let (_handle, output) = connect(config);

    let mut server = accept(&listener).await;
    server.handshake().await;
    server.hello("conn1").await;
    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert_eq!(event_name(&next(&output).await), "Hello");

    let ping = server.recv().await;
    assert_eq!(ping["action"], "ping");
    server
        .send(json!({
            "status": "FAIL",
            "seq_reply": ping["seq"],
            "error": {
                "id": "api.web_socket_router.not_authenticated.app_error",
                "message": "WebSocket connection is not authenticated. Please log in and try again.",
                "status_code": 401,
            },
        }))
        .await;
    assert_eq!(server.expect_close().await, Some(NORMAL_CLOSE_CODE));
    assert!(matches!(next(&output).await, WsOutput::AuthFailed));
    assert!(timeout(WAIT, output.recv()).await.unwrap().is_err());
}

#[tokio::test]
async fn connect_failures_back_off_and_reconnect_now_skips_the_wait() {
    // Nothing listens on this port at first.
    let (listener, addr) = listen().await;
    drop(listener);
    let config = WsConfig {
        backoff: Backoff {
            min: Duration::from_secs(3600),
            ..Backoff::default()
        },
        ..config(addr)
    };
    let (handle, output) = connect(config);
    match next(&output).await {
        WsOutput::Disconnected {
            reason: DisconnectReason::ConnectFailed(_),
            retry_in,
            session_confirmed: false,
        } => assert!(retry_in >= Duration::from_secs(3600)),
        other => panic!("expected a connect failure, got {other:?}"),
    }

    let listener = TcpListener::bind(addr).await.unwrap();
    handle.reconnect_now();
    let mut server = accept(&listener).await;
    server.handshake().await;
    server.hello("conn1").await;
    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    assert_eq!(event_name(&next(&output).await), "Hello");

    handle.reconnect_now();
    assert_eq!(server.expect_close().await, Some(NORMAL_CLOSE_CODE));
    assert!(matches!(
        next(&output).await,
        WsOutput::Disconnected {
            reason: DisconnectReason::ReconnectRequested,
            retry_in: Duration::ZERO,
            session_confirmed: true,
        }
    ));
    let again = accept(&listener).await;
    assert!(
        again.uri.contains("disconnect_err_code=1000"),
        "{}",
        again.uri
    );
}

#[tokio::test]
async fn unconfirmed_connections_count_as_failures() {
    // With an invalid token the server accepts the upgrade, then drops the socket when the
    // authentication challenge fails.
    let (listener, addr) = listen().await;
    let config = WsConfig {
        backoff: Backoff {
            min: Duration::from_millis(10),
            max: Duration::from_secs(10),
            fails_before_growth: 1,
            jitter: Duration::ZERO,
        },
        ..config(addr)
    };
    let (_handle, output) = connect(config);

    let mut retries = Vec::new();
    for _ in 0..2 {
        let mut server = accept(&listener).await;
        assert_eq!(server.recv().await["action"], "authentication_challenge");
        drop(server);
        assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
        match next(&output).await {
            WsOutput::Disconnected {
                retry_in,
                session_confirmed: false,
                ..
            } => retries.push(retry_in),
            other => panic!("expected an unconfirmed disconnect, got {other:?}"),
        }
    }
    assert_eq!(
        retries,
        [Duration::from_millis(10), Duration::from_millis(40)],
        "the second failure backs off further"
    );

    // A confirmed session resets the failure count.
    let mut server = accept(&listener).await;
    server.handshake().await;
    assert!(matches!(next(&output).await, WsOutput::Connected { .. }));
    server.ws.close(None).await.unwrap();
    assert!(matches!(
        next(&output).await,
        WsOutput::Disconnected {
            retry_in,
            session_confirmed: true,
            ..
        } if retry_in == Duration::from_millis(10)
    ));
}
