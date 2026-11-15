// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{
    Channel, ChannelMember, OrderedSidebarCategories, Preference, Status, Team, TeamMember, User,
};
use serde_json::json;

use super::*;

const ME: &str = "meeeeeeeeeeeeeeeeeeeeeeeee";
const BOB: &str = "bobbbbbbbbbbbbbbbbbbbbbbbb";

fn channel(id: &str, team: &str, kind: &str, name: &str, display: &str, total: i64) -> Channel {
    serde_json::from_value(json!({
        "id": id, "team_id": team, "type": kind, "name": name, "display_name": display,
        "total_msg_count": total, "total_msg_count_root": total, "last_post_at": total,
    }))
    .unwrap()
}

fn member(channel: &str, read: i64, mentions: i64) -> ChannelMember {
    serde_json::from_value(json!({
        "channel_id": channel, "user_id": ME, "msg_count": read, "msg_count_root": read,
        "mention_count": mentions, "mention_count_root": mentions,
        "notify_props": {"mark_unread": "all"},
    }))
    .unwrap()
}

fn category(
    id: &str,
    team: &str,
    kind: &str,
    name: &str,
    collapsed: bool,
    channels: &[&str],
) -> serde_json::Value {
    json!({"id": id, "team_id": team, "user_id": ME, "type": kind, "display_name": name,
           "collapsed": collapsed, "sorting": "manual", "channel_ids": channels})
}

/// Team "Beta" (t1) with #town-square (read), private #dev (2 unread, 1 mention), a DM with
/// bob (online) and a GM, and a collapsed Favorites holding #dev and #quiet; team "Alpha" (t2)
/// with one unread channel.
fn store() -> Store {
    let mut store = Store::new(User {
        id: ME.into(),
        username: "me".into(),
        ..User::default()
    });
    let team = |id: &str, name: &str| -> Team {
        serde_json::from_value(json!({"id": id, "display_name": name, "name": name})).unwrap()
    };
    let team_member = |id: &str| -> TeamMember {
        serde_json::from_value(json!({"team_id": id, "user_id": ME})).unwrap()
    };
    store.set_teams(
        vec![team("t1", "Beta"), team("t2", "Alpha")],
        vec![team_member("t1"), team_member("t2")],
    );
    store.set_channels(
        vec![
            channel("town", "t1", "O", "town-square", "Town Square", 3),
            channel("dev", "t1", "P", "dev", "Dev", 5),
            channel("quiet", "t1", "O", "quiet", "Quiet", 1),
            channel("dm", "", "D", &format!("{BOB}__{ME}"), "", 2),
            channel("gm", "", "G", "gmhash", "bob, carol, me", 1),
            channel("other", "t2", "O", "other", "Other", 4),
        ],
        vec![
            member("town", 3, 0),
            member("dev", 3, 1),
            member("quiet", 1, 0),
            member("dm", 2, 0),
            member("gm", 1, 0),
            member("other", 1, 0),
        ],
    );
    store.users.insert(
        BOB.into(),
        std::sync::Arc::new(User {
            id: BOB.into(),
            username: "bob".into(),
            ..User::default()
        }),
    );
    store.set_statuses(vec![Status {
        user_id: BOB.into(),
        status: "online".into(),
        ..Status::default()
    }]);
    // DMs and GMs show only when opted in (or unread).
    let show = |category: &str, name: &str| Preference {
        user_id: ME.into(),
        category: category.into(),
        name: name.into(),
        value: "true".into(),
    };
    store.set_preferences(vec![
        show("direct_channel_show", BOB),
        show("group_channel_show", "gm"),
    ]);
    let ordered = |categories: Vec<serde_json::Value>| -> OrderedSidebarCategories {
        let order: Vec<_> = categories.iter().map(|c| c["id"].clone()).collect();
        serde_json::from_value(json!({"categories": categories, "order": order})).unwrap()
    };
    store.set_categories(
        "t1".into(),
        ordered(vec![
            category(
                "fav",
                "t1",
                "favorites",
                "Favorites",
                true,
                &["dev", "quiet"],
            ),
            category("chan", "t1", "channels", "Channels", false, &["town"]),
            category(
                "dms",
                "t1",
                "direct_messages",
                "Direct Messages",
                false,
                &["dm", "gm"],
            ),
        ]),
    );
    store.set_categories(
        "t2".into(),
        ordered(vec![category(
            "chan2",
            "t2",
            "channels",
            "Channels",
            false,
            &["other"],
        )]),
    );
    store
}

fn names(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Header {
                title, collapsed, ..
            } => format!("{}{title}", if *collapsed { "+" } else { "-" }),
            Row::Channel(channel) => channel.name.clone(),
            Row::Empty(text) => text.clone(),
        })
        .collect()
}

#[test]
fn teams_follow_the_saved_order_then_names() {
    let mut store = store();
    let teams = teams(&store);
    assert_eq!(teams[0].name, "Alpha");
    assert_eq!(teams[1].name, "Beta");
    assert_eq!(teams[1].unread.mentions, 1);
    store
        .preferences
        .insert((TEAMS_ORDER.into(), String::new()), "t1,t2".into());
    let ordered = super::teams(&store);
    assert_eq!(ordered[0].name, "Beta");
    // The switcher's badge sums the teams not shown.
    let others = other_teams_unread(&ordered, Some(&"t1".into()));
    assert_eq!((others.messages, others.mentions), (3, 0));
}

#[test]
fn collapsed_categories_keep_only_unread_and_current_chats() {
    let store = store();
    let rows = rows(&store, &"t1".into(), None, "");
    assert_eq!(
        names(&rows),
        [
            "+Favorites",
            "Dev",
            "-Channels",
            "Town Square",
            "-Direct Messages",
            "bob",
            "bob, carol"
        ]
    );
    let dev = rows[1].channel().unwrap();
    assert_eq!(dev.icon, ChannelIcon::Private);
    assert_eq!((dev.unread.messages, dev.unread.mentions), (2, 1));
    assert_eq!(
        rows[5].channel().unwrap().icon,
        ChannelIcon::Direct(Presence::Online)
    );
    assert_eq!(rows[6].channel().unwrap().icon, ChannelIcon::Group(2));

    // The current chat stays visible in a collapsed category.
    let rows = super::rows(&store, &"t1".into(), Some(&"quiet".into()), "");
    assert_eq!(names(&rows)[..3], ["+Favorites", "Dev", "Quiet"]);
    assert!(rows[2].channel().unwrap().current);
}

#[test]
fn the_filter_lists_matching_chats_of_every_category() {
    let store = store();
    let rows = rows(&store, &"t1".into(), None, "  QUI ");
    assert_eq!(names(&rows), ["Quiet"]);
    // The channel's URL name matches too.
    assert_eq!(
        names(&super::rows(&store, &"t1".into(), None, "town-sq")),
        ["Town Square"]
    );
    assert_eq!(
        names(&super::rows(&store, &"t1".into(), None, "bob")),
        ["bob", "bob, carol"]
    );
    assert_eq!(
        names(&super::rows(&store, &"t1".into(), None, "zzz")),
        ["No chats match \"zzz\"."]
    );
}

#[test]
fn alt_arrows_move_through_chats_and_unread_ones() {
    let store = store();
    let rows = rows(&store, &"t1".into(), None, "");
    let id = |id: &str| Some(ChannelId::from(id));
    assert_eq!(step(&rows, None, 1, false), id("dev"));
    assert_eq!(step(&rows, None, -1, false), id("gm"));
    assert_eq!(step(&rows, id("dev").as_ref(), 1, false), id("town"));
    assert_eq!(step(&rows, id("gm").as_ref(), 1, false), id("dev"), "wraps");
    // Only #dev is unread: from it, there is no other unread chat.
    assert_eq!(step(&rows, id("town").as_ref(), 1, true), id("dev"));
    assert_eq!(step(&rows, id("dev").as_ref(), -1, true), None);
}

#[test]
fn an_empty_favorites_category_is_hidden() {
    let mut store = store();
    let mut categories = store.categories[&TeamId::from("t1")].clone();
    categories[0].channel_ids.clear();
    let order = categories
        .iter()
        .map(|category| category.id.clone())
        .collect();
    store.set_categories("t1".into(), OrderedSidebarCategories { categories, order });
    let rows = rows(&store, &"t1".into(), None, "");
    assert_eq!(names(&rows)[0], "-Channels");
}
