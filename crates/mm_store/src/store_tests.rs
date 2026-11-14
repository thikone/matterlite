// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

fn user(id: &str, username: &str, first: &str, last: &str) -> User {
    User {
        id: id.into(),
        username: username.into(),
        first_name: first.into(),
        last_name: last.into(),
        ..User::default()
    }
}

fn store() -> Store {
    let mut store = Store::new(user("aaa", "alice", "Alice", "Anderson"));
    store.add_users(vec![
        user("bbb", "bob", "Bob", "Brown"),
        user("ccc", "carol", "Carol", "Chen"),
    ]);
    let channels = vec![
        Channel {
            id: "dm".into(),
            channel_type: ChannelType::Direct,
            name: "aaa__bbb".into(),
            ..Channel::default()
        },
        Channel {
            id: "self".into(),
            channel_type: ChannelType::Direct,
            name: "aaa__aaa".into(),
            ..Channel::default()
        },
        Channel {
            id: "gm".into(),
            channel_type: ChannelType::Group,
            display_name: "alice, bob, carol, zed".into(),
            ..Channel::default()
        },
        Channel {
            id: "not-mine".into(),
            ..Channel::default()
        },
    ];
    let members = ["dm", "self", "gm"]
        .into_iter()
        .map(|id| ChannelMember {
            channel_id: id.into(),
            user_id: "aaa".into(),
            ..ChannelMember::default()
        })
        .collect();
    store.set_channels(channels, members);
    store
}

#[test]
fn channels_without_my_membership_are_dropped() {
    let store = store();
    assert_eq!(store.channels.len(), 3);
    assert!(!store.channels.contains_key(&ChannelId::from("not-mine")));
}

#[test]
fn dm_and_gm_names_follow_the_name_format() {
    let mut store = store();
    assert_eq!(store.channel_name(&"dm".into()).as_deref(), Some("bob"));
    assert_eq!(store.channel_name(&"self".into()).as_deref(), Some("alice"));
    // `zed` has no profile loaded: the username stays.
    assert_eq!(
        store.channel_name(&"gm".into()).as_deref(),
        Some("bob, carol, zed")
    );

    store.set_preferences(vec![Preference {
        user_id: "aaa".into(),
        category: "display_settings".into(),
        name: "name_format".into(),
        value: "full_name".into(),
    }]);
    assert_eq!(
        store.channel_name(&"dm".into()).as_deref(),
        Some("Bob Brown")
    );
    assert_eq!(
        store.channel_name(&"gm".into()).as_deref(),
        Some("Bob Brown, Carol Chen, zed")
    );
}

#[test]
fn a_licensed_server_can_lock_the_name_format() {
    let mut store = store();
    store.set_preferences(vec![Preference {
        category: "display_settings".into(),
        name: "name_format".into(),
        value: "full_name".into(),
        ..Preference::default()
    }]);
    let config = |value: serde_json::Value| serde_json::from_value::<ClientConfig>(value).unwrap();
    store.set_config(config(json!({
        "TeammateNameDisplay": "username", "LockTeammateNameDisplay": "true"
    })));
    assert_eq!(
        store.name_format(),
        NameFormat::FullName,
        "unlicensed: no lock"
    );
    store.set_license(config(json!({"LockTeammateNameDisplay": "true"})));
    assert_eq!(store.name_format(), NameFormat::Username);
}

#[test]
fn categories_follow_the_server_order() {
    let mut store = store();
    let ordered: OrderedSidebarCategories = serde_json::from_value(json!({
        "categories": [
            {"id": "dms", "type": "direct_messages", "sort_order": 20},
            {"id": "fav", "type": "favorites", "sort_order": 0},
            {"id": "custom", "type": "custom", "sort_order": 30},
            {"id": "chan", "type": "channels", "sort_order": 10}
        ],
        "order": ["fav", "chan", "dms"]
    }))
    .unwrap();
    store.set_categories("t1".into(), ordered);
    let ids: Vec<&str> = store.categories[&TeamId::from("t1")]
        .iter()
        .map(|category| category.id.as_str())
        .collect();
    assert_eq!(ids, ["fav", "chan", "dms", "custom"]);
}

#[test]
fn permissions_come_from_all_my_roles() {
    let mut store = store();
    store.set_roles(vec![
        Role {
            name: "channel_user".into(),
            permissions: vec!["create_post".into()],
            ..Role::default()
        },
        Role {
            name: "system_user".into(),
            permissions: vec!["create_direct_channel".into()],
            ..Role::default()
        },
    ]);
    store
        .channel_members
        .get_mut(&ChannelId::from("dm"))
        .unwrap()
        .roles = "channel_user".into();
    Arc::make_mut(store.users.get_mut(&UserId::from("aaa")).unwrap()).roles = "system_user".into();
    assert!(store.has_channel_permission(&"dm".into(), "create_post"));
    assert!(store.has_channel_permission(&"gm".into(), "create_direct_channel"));
    assert!(!store.has_channel_permission(&"gm".into(), "create_post"));
}
