// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn channels_fixture() {
    let channels: Vec<Channel> =
        serde_json::from_str(include_str!("../fixtures/channels.json")).unwrap();

    let town_square = &channels[0];
    assert_eq!(town_square.channel_type, ChannelType::Open);
    assert_eq!(town_square.total_msg_count_root, 380);
    assert!(town_square.props.is_empty());
    assert_eq!(town_square.scheme_id, None);

    let direct = &channels[1];
    assert!(direct.channel_type.is_direct_or_group());
    assert!(direct.team_id.is_empty());

    let future = &channels[2];
    assert_eq!(future.channel_type, ChannelType::Other("X".to_owned()));
    let text = serde_json::to_string(future).unwrap();
    assert!(text.contains(r#""type":"X""#), "{text}");
}

#[test]
fn member_counters_and_mute() {
    let member: ChannelMember = serde_json::from_str(
        r#"{"channel_id": "c", "user_id": "u", "roles": "channel_user", "last_viewed_at": 10,
            "msg_count": 4, "mention_count": 1, "mention_count_root": 1,
            "urgent_mention_count": 0, "msg_count_root": 3,
            "notify_props": {"desktop": "default", "mark_unread": "mention", "push": "all"},
            "last_update_at": 11, "scheme_guest": false, "scheme_user": true,
            "scheme_admin": false, "explicit_roles": ""}"#,
    )
    .unwrap();
    assert!(member.is_muted());
    assert_eq!(member.msg_count_root, 3);
}

#[test]
fn categories_with_null_channel_ids() {
    let categories: OrderedSidebarCategories = serde_json::from_str(
        r#"{"categories": [{"id": "favorites_u_t", "user_id": "u", "team_id": "t",
              "sort_order": 0, "sorting": "", "type": "favorites",
              "display_name": "Favorites", "muted": false, "collapsed": false,
              "channel_ids": null}],
            "order": ["favorites_u_t"]}"#,
    )
    .unwrap();
    assert!(categories.categories[0].channel_ids.is_empty());
    assert_eq!(categories.order[0].as_str(), "favorites_u_t");
}
