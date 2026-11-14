// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn normalizes_what_users_type() {
    assert_eq!(
        ServerUrl::parse("chat.example.com").unwrap().as_str(),
        "https://chat.example.com"
    );
    assert_eq!(
        ServerUrl::parse(" https://example.com/mattermost/?x=1#top ")
            .unwrap()
            .as_str(),
        "https://example.com/mattermost"
    );
    assert_eq!(
        ServerUrl::parse("http://localhost:8065/").unwrap().as_str(),
        "http://localhost:8065"
    );
    assert!(matches!(
        ServerUrl::parse("ftp://example.com"),
        Err(ServerUrlError::Scheme)
    ));
}

#[test]
fn api_urls_keep_the_subpath_and_encode_segments() {
    let server = ServerUrl::parse("https://example.com/mattermost").unwrap();
    assert_eq!(
        server.api_url(&["users", "me"]).as_str(),
        "https://example.com/mattermost/api/v4/users/me"
    );
    assert_eq!(
        server
            .api_url(&["teams", "name", "a b", "channels", "name", "x/y"])
            .as_str(),
        "https://example.com/mattermost/api/v4/teams/name/a%20b/channels/name/x%2Fy"
    );
    let root = ServerUrl::parse("http://localhost:8065").unwrap();
    assert_eq!(
        root.api_url(&["system", "ping"]).as_str(),
        "http://localhost:8065/api/v4/system/ping"
    );
}

#[test]
fn websocket_urls() {
    let server = ServerUrl::parse("https://example.com/mm").unwrap();
    assert_eq!(
        server.websocket_url(None).unwrap().as_str(),
        "wss://example.com/mm/api/v4/websocket"
    );
    let local = ServerUrl::parse("http://localhost:8065").unwrap();
    assert_eq!(
        local.websocket_url(None).unwrap().as_str(),
        "ws://localhost:8065/api/v4/websocket"
    );
    assert_eq!(
        local
            .websocket_url(Some("wss://ws.example.com:4443"))
            .unwrap()
            .as_str(),
        "wss://ws.example.com:4443/api/v4/websocket"
    );
    assert_eq!(
        local
            .websocket_url(Some("https://push.example.com"))
            .unwrap()
            .as_str(),
        "wss://push.example.com/api/v4/websocket"
    );
}
