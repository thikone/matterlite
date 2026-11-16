// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, OrderedSidebarCategories, Status, Team, TeamMember, User};
use serde_json::json;

use super::*;

const ME: &str = "meeeeeeeeeeeeeeeeeeeeeeeee";
const BOB: &str = "bobbbbbbbbbbbbbbbbbbbbbbbb";

fn store() -> Store {
    let mut store = Store::new(User {
        id: ME.into(),
        username: "me".into(),
        ..User::default()
    });
    store.add_users(vec![User {
        id: BOB.into(),
        username: "bob".into(),
        ..User::default()
    }]);
    let team: Team = serde_json::from_value(json!({"id": "t1", "name": "acme"})).unwrap();
    let team_member: TeamMember =
        serde_json::from_value(json!({"team_id": "t1", "user_id": ME})).unwrap();
    store.set_teams(vec![team], vec![team_member]);
    let channel = |id: &str, team: &str, kind: &str, name: &str| -> Channel {
        serde_json::from_value(json!({"id": id, "team_id": team, "type": kind, "name": name}))
            .unwrap()
    };
    let member = |id: &str, mark_unread: &str| -> ChannelMember {
        serde_json::from_value(json!({
            "channel_id": id, "user_id": ME, "notify_props": {"mark_unread": mark_unread}
        }))
        .unwrap()
    };
    store.set_channels(
        vec![
            channel("dev", "t1", "O", "dev"),
            channel("town", "t1", "O", "town-square"),
            channel("dm", "", "D", &format!("{BOB}__{ME}")),
        ],
        vec![
            member("dev", "mention"),
            member("town", "all"),
            member("dm", "all"),
        ],
    );
    let categories: OrderedSidebarCategories = serde_json::from_value(json!({
        "categories": [
            {"id": "fav", "team_id": "t1", "type": "favorites", "channel_ids": ["dev"]},
            {"id": "chan", "team_id": "t1", "type": "channels", "channel_ids": ["town"]},
            {"id": "dms", "team_id": "t1", "type": "direct_messages", "channel_ids": ["dm"]},
        ],
        "order": ["fav", "chan", "dms"],
    }))
    .unwrap();
    store.set_categories("t1".into(), categories);
    store
}

#[test]
fn favorites_move_between_categories() {
    let mut store = store();
    let dev = favorite(&store, &"dev".into(), None).unwrap();
    assert!(dev.is_favorite);
    assert_eq!(dev.target, CategoryId::new("chan"));
    // A DM has no team: the shown team's categories are used.
    assert_eq!(favorite(&store, &"dm".into(), None), None);
    let dm = favorite(&store, &"dm".into(), Some(&"t1".into())).unwrap();
    assert!(!dm.is_favorite);
    assert_eq!(dm.target, CategoryId::new("fav"));

    // Moving puts it at the top of the target and takes it out of the other one.
    let changed = store.move_to_category(&"t1".into(), &"dm".into(), &"fav".into());
    assert_eq!(changed.len(), 2);
    let fav = favorite(&store, &"dm".into(), Some(&"t1".into())).unwrap();
    assert!(fav.is_favorite);
    assert_eq!(fav.target, CategoryId::new("dms"));
    assert_eq!(
        store.categories[&TeamId::new("t1")][0].channel_ids,
        [ChannelId::new("dm"), ChannelId::new("dev")]
    );
}

#[test]
fn muted_leave_and_links() {
    let mut store = store();
    assert!(is_muted(&store, &"dev".into()));
    assert!(!is_muted(&store, &"town".into()));
    store.set_muted(&"dev".into(), false);
    assert!(!is_muted(&store, &"dev".into()));

    assert!(can_leave(&store, &"dev".into()));
    assert!(!can_leave(&store, &"town".into()), "Town Square stays");
    assert!(!can_leave(&store, &"dm".into()));

    assert_eq!(
        link(&store, &"dev".into(), None, "https://chat.example/").as_deref(),
        Some("https://chat.example/acme/channels/dev")
    );
    assert_eq!(
        link(
            &store,
            &"dm".into(),
            Some(&"t1".into()),
            "https://chat.example"
        )
        .as_deref(),
        Some("https://chat.example/acme/messages/@bob")
    );
    store.set_config(serde_json::from_value(json!({"SiteURL": "https://mm.example"})).unwrap());
    assert_eq!(
        link(&store, &"dev".into(), None, "https://chat.example").as_deref(),
        Some("https://mm.example/acme/channels/dev")
    );
}

#[test]
fn members_online_first_then_by_name() {
    let mut store = store();
    assert_eq!(members(&store, &"dev".into()), None, "not loaded yet");
    let user = |id: &str, name: &str| User {
        id: id.into(),
        username: name.into(),
        ..User::default()
    };
    store.set_channel_users(
        "dev".into(),
        vec![
            user("u1", "zed"),
            user("u2", "amy"),
            user("u3", "carl"),
            user("u4", "bea"),
        ],
    );
    let status = |id: &str, status: &str| Status {
        user_id: id.into(),
        status: status.into(),
        ..Status::default()
    };
    store.set_statuses(vec![
        status("u1", "online"),
        status("u3", "dnd"),
        status("u4", "online"),
    ]);
    let names: Vec<String> = members(&store, &"dev".into())
        .unwrap()
        .into_iter()
        .map(|row| row.username)
        .collect();
    assert_eq!(names, ["bea", "zed", "carl", "amy"]);
}
