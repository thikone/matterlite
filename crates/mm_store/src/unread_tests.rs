// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

fn channel(total: i64, root: i64) -> Channel {
    Channel {
        total_msg_count: total,
        total_msg_count_root: root,
        ..Channel::default()
    }
}

fn member(msgs: i64, root: i64, mentions: i64, mentions_root: i64) -> ChannelMember {
    ChannelMember {
        msg_count: msgs,
        msg_count_root: root,
        mention_count: mentions,
        mention_count_root: mentions_root,
        ..ChannelMember::default()
    }
}

#[test]
fn crt_uses_the_root_counters() {
    // #dev on the test server: 11 posts unread, 8 of them roots, 2 mentions.
    let channel = channel(20, 15);
    let member = member(9, 7, 3, 2);
    assert_eq!(
        ChannelUnread::compute(&channel, &member, true),
        ChannelUnread {
            messages: 8,
            mentions: 2,
            urgent_mentions: 0,
            muted: false
        }
    );
    assert_eq!(
        ChannelUnread::compute(&channel, &member, false).messages,
        11
    );
}

#[test]
fn muted_channels_count_only_mentions() {
    let mut member = member(0, 0, 0, 0);
    member
        .notify_props
        .insert("mark_unread".into(), "mention".into());
    let quiet = ChannelUnread::compute(&channel(5, 5), &member, true);
    assert!(quiet.muted && quiet.messages == 5 && !quiet.is_unread());

    member.mention_count_root = 1;
    let mentioned = ChannelUnread::compute(&channel(5, 5), &member, true);
    assert!(mentioned.is_unread());

    let mut team = TeamUnread::default();
    team.add_channel(quiet);
    team.add_channel(mentioned);
    assert_eq!((team.messages, team.mentions), (0, 1));
}

#[test]
fn which_posts_count() {
    let post = |value: serde_json::Value| -> Post { serde_json::from_value(value).unwrap() };
    assert!(counts_as_message(&post(json!({"message": "hi"}))));
    assert!(!counts_as_message(&post(
        json!({"type": "system_join_channel"})
    )));
    assert!(counts_as_message(&post(
        json!({"type": "system_header_change"})
    )));
    assert!(!counts_as_message(&post(
        json!({"props": {"silent_notification": true}})
    )));
    assert!(counts_as_message(&post(
        json!({"props": {"silent_notification": true, "force_notification": true}})
    )));

    let me = UserId::new("me");
    assert!(is_own_message(&post(json!({"user_id": "me"})), &me));
    assert!(!is_own_message(
        &post(json!({"user_id": "me", "type": "system_header_change"})),
        &me
    ));
    assert!(!is_own_message(
        &post(json!({"user_id": "me", "props": {"from_webhook": "true"}})),
        &me
    ));
    assert!(is_urgent(&post(
        json!({"metadata": {"priority": {"priority": "urgent"}}})
    )));
}
