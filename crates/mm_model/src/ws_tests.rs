// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

fn event(text: &str) -> EventFrame {
    match ServerFrame::parse(text).unwrap() {
        ServerFrame::Event(frame) => frame,
        ServerFrame::Response(response) => panic!("expected an event, got {response:?}"),
    }
}

fn decode(value: Value) -> WsEvent {
    WsEvent::decode(&event(&value.to_string())).unwrap()
}

#[test]
fn responses_and_events_are_told_apart() {
    let frame = ServerFrame::parse(r#"{"status": "OK", "seq_reply": 2, "data": {"text": "pong"}}"#)
        .unwrap();
    let ServerFrame::Response(response) = frame else {
        panic!("expected a response");
    };
    assert!(response.is_ok());
    assert_eq!(response.seq_reply, 2);

    let failed = ServerFrame::parse(
        r#"{"status": "FAIL", "seq_reply": 3,
            "error": {"id": "api.web_socket_router.bad_action.app_error", "message": "x"}}"#,
    )
    .unwrap();
    let ServerFrame::Response(failed) = failed else {
        panic!("expected a response");
    };
    assert!(!failed.is_ok());
    assert_eq!(
        failed.error.unwrap().id,
        "api.web_socket_router.bad_action.app_error"
    );
}

#[test]
fn hello() {
    let frame = event(
        r#"{"event": "hello", "data": {"connection_id": "conn1", "server_hostname": "mm1",
            "server_version": "10.11.0.123.abc.false"},
            "broadcast": {"omit_users": null, "user_id": "me", "channel_id": "", "team_id": "",
            "connection_id": "", "omit_connection_id": ""}, "seq": 0}"#,
    );
    assert_eq!(frame.seq, 0);
    assert_eq!(frame.broadcast.user_id.as_str(), "me");
    let WsEvent::Hello(hello) = WsEvent::decode(&frame).unwrap() else {
        panic!("expected hello");
    };
    assert_eq!(hello.connection_id, "conn1");
    assert_eq!(hello.server_hostname, "mm1");
}

#[test]
fn posted_with_string_encoded_fields() {
    let post = json!({"id": "p1", "channel_id": "c1", "user_id": "u1", "message": "hi @me",
                      "pending_post_id": "u1:1763112000000", "props": {}});
    let event = decode(json!({
        "event": "posted",
        "data": {
            "post": post.to_string(),
            "channel_type": "O",
            "channel_name": "town-square",
            "channel_display_name": "Town Square",
            "sender_name": "@alice",
            "team_id": "t1",
            "set_online": true,
            "mentions": "[\"me\"]",
            "otherFile": "true"
        },
        "broadcast": {"channel_id": "c1"},
        "seq": 5
    }));
    let WsEvent::Posted(posted) = event else {
        panic!("expected posted");
    };
    assert_eq!(posted.post.message, "hi @me");
    assert_eq!(posted.post.pending_post_id, "u1:1763112000000");
    assert_eq!(posted.mentions, [UserId::from("me")]);
    assert!(posted.followers.is_empty());
    assert!(posted.set_online);
}

#[test]
fn nested_documents_may_also_arrive_unencoded() {
    let event = decode(json!({
        "event": "post_edited",
        "data": {"post": {"id": "p1", "message": "edited", "edit_at": 7}},
        "seq": 1
    }));
    let WsEvent::PostEdited(post) = event else {
        panic!("expected post_edited");
    };
    assert_eq!(post.edit_at, 7);
}

#[test]
fn a_missing_entity_is_a_decode_error() {
    let frame = event(r#"{"event": "post_edited", "data": {}, "seq": 1}"#);
    let error = WsEvent::decode(&frame).unwrap_err();
    assert_eq!(error.event, "post_edited");
    assert!(error.to_string().contains("missing `post`"), "{error}");

    let frame = event(r#"{"event": "posted", "data": {"post": "{not json"}, "seq": 1}"#);
    assert!(WsEvent::decode(&frame).is_err());
}

#[test]
fn null_data_is_an_empty_object() {
    let frame = event(r#"{"event": "thread_read_changed", "data": null, "seq": 3}"#);
    assert_eq!(
        WsEvent::decode(&frame).unwrap(),
        WsEvent::ThreadReadChanged(ThreadReadChanged::default())
    );
}

#[test]
fn member_preferences_categories_and_counts() {
    let member = json!({"channel_id": "c1", "user_id": "me", "msg_count": 9,
                        "notify_props": {"mark_unread": "all"}});
    let WsEvent::ChannelMemberUpdated(member) = decode(json!({
        "event": "channel_member_updated",
        "data": {"channelMember": member.to_string()},
        "seq": 1
    })) else {
        panic!("expected channel_member_updated");
    };
    assert_eq!(member.msg_count, 9);

    let preferences = json!([{"user_id": "me", "category": "display_settings",
                              "name": "collapsed_reply_threads", "value": "on"}]);
    assert_eq!(
        decode(json!({
            "event": "preferences_changed",
            "data": {"preferences": preferences.to_string()},
            "seq": 2
        })),
        WsEvent::PreferencesChanged(vec![Preference {
            user_id: "me".into(),
            category: "display_settings".into(),
            name: "collapsed_reply_threads".into(),
            value: "on".into(),
        }])
    );

    let single = json!({"user_id": "me", "category": "theme", "name": "", "value": "{}"});
    let WsEvent::PreferencesChanged(single) = decode(json!({
        "event": "preference_changed",
        "data": {"preference": single.to_string()},
        "seq": 3
    })) else {
        panic!("expected preference_changed");
    };
    assert_eq!(single[0].category, "theme");

    let categories = json!([{"id": "custom_1", "type": "custom", "channel_ids": ["c1", "c2"]}]);
    let WsEvent::SidebarCategoryUpdated(categories) = decode(json!({
        "event": "sidebar_category_updated",
        "data": {"updatedCategories": categories.to_string()},
        "seq": 4
    })) else {
        panic!("expected sidebar_category_updated");
    };
    assert_eq!(categories[0].channel_ids.len(), 2);

    let WsEvent::SidebarCategoryOrderUpdated(order) = decode(json!({
        "event": "sidebar_category_order_updated",
        "data": {"order": ["favorites_me_t1", "custom_1"]},
        "seq": 5
    })) else {
        panic!("expected sidebar_category_order_updated");
    };
    assert_eq!(order.order.len(), 2);

    let WsEvent::MultipleChannelsViewed(viewed) = decode(json!({
        "event": "multiple_channels_viewed",
        "data": {"channel_times": {"c1": 100, "c2": 200}},
        "seq": 6
    })) else {
        panic!("expected multiple_channels_viewed");
    };
    assert_eq!(viewed.channel_times["c2"], 200);

    let WsEvent::GroupAdded(group) = decode(json!({
        "event": "group_added",
        "data": {"teammate_ids": "[\"u1\",\"u2\"]"},
        "seq": 7
    })) else {
        panic!("expected group_added");
    };
    assert_eq!(group.teammate_ids.len(), 2);
}

#[test]
fn user_updated_and_config_are_plain_objects() {
    let WsEvent::UserUpdated(user) = decode(json!({
        "event": "user_updated",
        "data": {"user": {"id": "u1", "username": "alice", "notify_props": null}},
        "seq": 1
    })) else {
        panic!("expected user_updated");
    };
    assert_eq!(user.username, "alice");

    let WsEvent::ConfigChanged(config) = decode(json!({
        "event": "config_changed",
        "data": {"config": {"EnableLatex": "true"}},
        "seq": 2
    })) else {
        panic!("expected config_changed");
    };
    assert!(config.bool("EnableLatex"));
}

#[test]
fn unknown_events_are_kept_by_name() {
    assert_eq!(
        decode(json!({"event": "scheduled_post_created", "data": {"x": 1}, "seq": 9})),
        WsEvent::Unknown("scheduled_post_created".to_owned())
    );
}

#[test]
fn client_action_frames() {
    let typing = ClientAction::UserTyping {
        channel_id: "c1".into(),
        parent_id: PostId::default(),
    };
    let frame: Value = serde_json::from_str(&typing.to_frame(3)).unwrap();
    assert_eq!(
        frame,
        json!({"action": "user_typing", "seq": 3, "data": {"channel_id": "c1", "parent_id": ""}})
    );

    let ping: Value = serde_json::from_str(&ClientAction::Ping.to_frame(1)).unwrap();
    assert_eq!(ping, json!({"action": "ping", "seq": 1}));

    let challenge = ClientAction::AuthenticationChallenge(SessionToken::new("secret"));
    let frame: Value = serde_json::from_str(&challenge.to_frame(1)).unwrap();
    assert_eq!(frame["data"]["token"], "secret");
    assert!(!format!("{challenge:?}").contains("secret"));

    let thread = ClientAction::Presence(Presence::Thread {
        channel_id: "c1".into(),
        is_thread_view: true,
    });
    let frame: Value = serde_json::from_str(&thread.to_frame(2)).unwrap();
    assert_eq!(
        frame["data"],
        json!({"thread_channel_id": "c1", "is_thread_view": true})
    );
}
