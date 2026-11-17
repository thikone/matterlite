// SPDX-License-Identifier: AGPL-3.0-only

use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{body_json, header, header_exists, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

async fn server() -> (MockServer, Client) {
    let mock = MockServer::start().await;
    let client = Client::new(
        ServerUrl::parse(&mock.uri()).unwrap(),
        ClientOptions::default(),
    )
    .unwrap();
    (mock, client)
}

fn logged_in(client: Client) -> Client {
    client.with_token(SessionToken::new("tok123"))
}

#[tokio::test]
async fn login_takes_the_token_from_the_header() {
    let (mock, client) = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .and(header("X-Requested-With", "XMLHttpRequest"))
        .and(body_json(json!({
            "login_id": "alice", "password": "pw", "token": "", "device_id": ""
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Token", "newtoken")
                .insert_header("X-Version-Id", "10.11.0.1.abc.false")
                .set_body_json(json!({"id": "u1", "username": "alice"})),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let response = client
        .login(&LoginRequest {
            login_id: "alice".into(),
            password: "pw".into(),
            ..LoginRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(response.token.expose(), "newtoken");
    assert_eq!(response.user.username, "alice");
    assert_eq!(
        client.server_version().as_deref(),
        Some("10.11.0.1.abc.false")
    );
    let requests = mock.received_requests().await.unwrap();
    assert!(requests[0].headers.get("Authorization").is_none());
}

#[tokio::test]
async fn login_without_mfa_code_asks_for_one() {
    let (mock, client) = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "mfa.validate_token.authenticate.app_error",
            "message": "Invalid MFA token.",
            "status_code": 401
        })))
        .mount(&mock)
        .await;
    let error = client.login(&LoginRequest::default()).await.unwrap_err();
    assert!(error.needs_mfa_code(), "{error:?}");
}

#[tokio::test]
async fn login_without_token_header_is_an_error() {
    let (mock, client) = server().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/users/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "u1"})))
        .mount(&mock)
        .await;
    assert!(matches!(
        client.login(&LoginRequest::default()).await,
        Err(ApiError::MissingToken)
    ));
}

#[tokio::test]
async fn authenticated_requests_send_the_bearer_token() {
    let (mock, client) = server().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/users/me"))
        .and(header("Authorization", "Bearer tok123"))
        .and(header("X-Requested-With", "XMLHttpRequest"))
        .and(header_exists("User-Agent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "u1", "roles": "system_user"})),
        )
        .expect(1)
        .mount(&mock)
        .await;
    let me = logged_in(client).me().await.unwrap();
    assert_eq!(me.role_names().collect::<Vec<_>>(), ["system_user"]);
}

#[tokio::test]
async fn errors() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    Mock::given(path("/api/v4/users/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "id": "api.context.session_expired.app_error",
            "message": "Invalid or expired session, please login again."
        })))
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/me/teams"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "3")
                .set_body_string("limit exceeded"),
        )
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/me/preferences"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad gateway</html>"))
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/me/teams/members"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&mock)
        .await;

    let expired = client.me().await.unwrap_err();
    assert!(expired.is_session_expired());
    assert_eq!(expired.app_error().unwrap().status_code, 401);

    assert!(matches!(
        client.my_teams().await,
        Err(ApiError::RateLimited { retry_after: Some(retry) }) if retry == Duration::from_secs(3)
    ));
    assert!(matches!(
        client.my_preferences().await,
        Err(ApiError::Status { status: 502, .. })
    ));
    assert!(matches!(
        client.my_team_members().await,
        Err(ApiError::Decode { .. })
    ));
}

#[tokio::test]
async fn channel_members_as_ndjson_or_array() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    let ndjson = "{\"channel_id\":\"c1\",\"user_id\":\"u1\",\"msg_count\":3}\n\
                  {\"channel_id\":\"c2\",\"user_id\":\"u1\",\"notify_props\":null}\n";
    Mock::given(path("/api/v4/users/u1/channel_members"))
        .and(query_param("page", "-1"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ndjson, "application/x-ndjson"))
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/u2/channel_members"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"channel_id": "c9"}])))
        .mount(&mock)
        .await;

    let members = client.my_channel_members(&"u1".into()).await.unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].msg_count, 3);
    assert_eq!(members[1].channel_id.as_str(), "c2");

    let members = client.my_channel_members(&"u2".into()).await.unwrap();
    assert_eq!(members[0].channel_id.as_str(), "c9");
}

#[tokio::test]
async fn a_user_in_no_channel_gets_empty_lists() {
    // Bodies as a live 12.2 server sends them: the channel list is streamed, so the error
    // follows the opening `[`.
    let (mock, client) = server().await;
    let client = logged_in(client);
    Mock::given(path("/api/v4/users/me/channels"))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            "[{\"id\":\"app.channel.get_channels.not_found.app_error\",\"message\":\"No channels were found.\",\"status_code\":404}
",
            "application/json",
        ))
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/u1/channel_members"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "app.channel.get_member.missing.app_error",
            "message": "No channel member found for that user ID and channel ID.",
            "status_code": 404
        })))
        .mount(&mock)
        .await;
    Mock::given(path("/api/v4/users/u2/channel_members"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "id": "api.user.get.app_error", "status_code": 404
        })))
        .mount(&mock)
        .await;

    assert!(client.my_channels().await.unwrap().is_empty());
    assert!(
        client
            .my_channel_members(&"u1".into())
            .await
            .unwrap()
            .is_empty()
    );
    // Other 404s stay errors.
    match client.my_channel_members(&"u2".into()).await {
        Err(ApiError::App(error)) => assert_eq!(error.id, "api.user.get.app_error"),
        other => panic!("expected an AppError, got {other:?}"),
    }
}

#[tokio::test]
async fn post_queries_and_view() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    Mock::given(method("GET"))
        .and(path("/api/v4/channels/c1/posts"))
        .and(query_param("before", "p9"))
        .and(query_param("per_page", "30"))
        .and(query_param("collapsedThreads", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order": ["p8"], "posts": {"p8": {"id": "p8", "message": "older"}},
            "next_post_id": "p9", "prev_post_id": ""
        })))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v4/channels/members/me/view"))
        .and(body_json(json!({
            "channel_id": "c1", "prev_channel_id": "", "collapsed_threads_supported": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&mock)
        .await;

    let page = client
        .channel_posts(
            &"c1".into(),
            &PostsQuery {
                before: Some("p9".into()),
                per_page: Some(30),
                collapsed_threads: true,
                ..PostsQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.ordered().next().unwrap().message, "older");
    assert!(page.prev_post_id.is_empty());
    client.view_channel(&"c1".into(), None).await.unwrap();
}

#[tokio::test]
async fn id_lists_are_json_arrays() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    Mock::given(method("POST"))
        .and(path("/api/v4/users/status/ids"))
        .and(body_json(json!(["u1", "u2"])))
        .respond_with(|request: &Request| {
            let ids: Vec<String> = serde_json::from_slice(&request.body).unwrap();
            let statuses: Vec<_> = ids
                .iter()
                .map(|id| json!({"user_id": id, "status": "online"}))
                .collect();
            ResponseTemplate::new(200).set_body_json(statuses)
        })
        .mount(&mock)
        .await;
    let statuses = client
        .statuses_by_ids(&["u1".into(), "u2".into()])
        .await
        .unwrap();
    assert_eq!(statuses[1].user_id.as_str(), "u2");
}

#[tokio::test]
async fn post_actions_use_the_webapp_routes() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    for (verb, route) in [
        ("POST", "/api/v4/posts/p1/pin"),
        ("POST", "/api/v4/posts/p1/unpin"),
        ("DELETE", "/api/v4/posts/p1"),
        ("PUT", "/api/v4/users/u1/teams/t1/threads/p1/following"),
        ("DELETE", "/api/v4/users/u1/teams/t1/threads/p1/following"),
        ("POST", "/api/v4/users/u1/teams/t1/threads/p1/set_unread/p2"),
        ("POST", "/api/v4/users/u1/preferences/delete"),
    ] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
            .expect(1)
            .mount(&mock)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/v4/users/u1/posts/p1/set_unread"))
        .and(body_json(json!({"collapsed_threads_supported": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v4/posts/p1/patch"))
        .and(body_json(json!({"message": "fixed"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "p1", "message": "fixed"})),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let (user, team, post) = (UserId::from("u1"), TeamId::from("t1"), PostId::from("p1"));
    client.pin_post(&post, true).await.unwrap();
    client.pin_post(&post, false).await.unwrap();
    client.delete_post(&post).await.unwrap();
    client
        .follow_thread(&user, &team, &post, true)
        .await
        .unwrap();
    client
        .follow_thread(&user, &team, &post, false)
        .await
        .unwrap();
    client
        .set_thread_unread(&user, &team, &post, &"p2".into())
        .await
        .unwrap();
    client.delete_preferences(&user, &[]).await.unwrap();
    client.set_post_unread(&user, &post, true).await.unwrap();
    assert_eq!(
        client.patch_post(&post, "fixed").await.unwrap().message,
        "fixed"
    );
}

#[tokio::test]
async fn reactions_use_the_webapp_routes() {
    let (mock, client) = server().await;
    let client = logged_in(client);
    Mock::given(method("POST"))
        .and(path("/api/v4/reactions"))
        .and(body_json(
            json!({"user_id": "u1", "post_id": "p1", "emoji_name": "+1"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"user_id": "u1", "post_id": "p1", "emoji_name": "+1", "create_at": 5}),
        ))
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v4/users/u1/posts/p1/reactions/+1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "OK"})))
        .expect(1)
        .mount(&mock)
        .await;
    let (user, post) = (UserId::from("u1"), PostId::from("p1"));
    let reaction = client.add_reaction(&user, &post, "+1").await.unwrap();
    assert_eq!(
        (reaction.emoji_name.as_str(), reaction.create_at),
        ("+1", 5)
    );
    client.remove_reaction(&user, &post, "+1").await.unwrap();
}
