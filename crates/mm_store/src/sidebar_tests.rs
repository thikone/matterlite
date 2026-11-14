// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{ChannelMember, OrderedSidebarCategories, User};
use serde_json::json;

use super::*;

const ME: &str = "me";

fn channel(id: &str, kind: &str, display_name: &str, last_post_at: i64) -> serde_json::Value {
    let team = if kind == "D" || kind == "G" { "" } else { "t1" };
    json!({"id": id, "type": kind, "team_id": team, "display_name": display_name, "name": id,
           "last_post_at": last_post_at, "last_root_post_at": last_post_at,
           "total_msg_count": 1, "total_msg_count_root": 1})
}

fn dm(id: &str, partner: &str, last_post_at: i64) -> serde_json::Value {
    let mut channel = channel(id, "D", "", last_post_at);
    let mut ids = [ME, partner];
    ids.sort_unstable();
    channel["name"] = json!(format!("{}__{}", ids[0], ids[1]));
    channel
}

/// Favorites `beta9`, `alpha` (manual); channels `Beta 10`, `beta 2`, a muted one and an
/// archived one; DMs with bob (shown), carol (unread, no preference), dave (read, no
/// preference), erin (deactivated after the last view), the self-DM, and a GM.
fn store() -> Store {
    let mut store = Store::new(User {
        id: ME.into(),
        username: "me".into(),
        ..User::default()
    });
    let user = |id: &str, delete_at: i64| User {
        id: id.into(),
        username: id.into(),
        delete_at,
        ..User::default()
    };
    store.add_users(vec![
        user("bob", 0),
        user("carol", 0),
        user("dave", 0),
        user("erin", 50),
    ]);
    let mut old = channel("old", "O", "old", 50);
    old["delete_at"] = json!(5);
    let channels: Vec<Channel> = serde_json::from_value(json!([
        channel("alpha", "O", "alpha", 10),
        channel("beta9", "P", "beta 9", 30),
        channel("beta10", "O", "Beta 10", 20),
        channel("beta2", "O", "beta 2", 25),
        channel("muted", "O", "aaa muted", 40),
        old,
        dm("dm-bob", "bob", 100),
        dm("dm-carol", "carol", 90),
        dm("dm-dave", "dave", 80),
        dm("dm-erin", "erin", 70),
        dm("dm-me", ME, 60),
        channel("gm", "G", "carol, bob, me", 110),
    ]))
    .unwrap();
    let members = channels
        .iter()
        .map(|channel| {
            let unread = channel.id.as_str() == "dm-carol";
            let mut member = ChannelMember {
                channel_id: channel.id.clone(),
                user_id: ME.into(),
                msg_count: i64::from(!unread),
                msg_count_root: i64::from(!unread),
                last_viewed_at: 10,
                ..ChannelMember::default()
            };
            if channel.id.as_str() == "muted" {
                member
                    .notify_props
                    .insert("mark_unread".into(), "mention".into());
            }
            member
        })
        .collect();
    store.set_channels(channels, members);
    store.set_preferences(
        serde_json::from_value(json!([
            {"category": "direct_channel_show", "name": "bob", "value": "true"},
            {"category": "direct_channel_show", "name": "erin", "value": "true"},
            {"category": "group_channel_show", "name": "gm", "value": "true"}
        ]))
        .unwrap(),
    );
    let categories: OrderedSidebarCategories = serde_json::from_value(json!({
        "categories": [
            {"id": "fav", "team_id": "t1", "type": "favorites", "sorting": "manual",
             "channel_ids": ["beta9", "alpha"]},
            {"id": "chan", "team_id": "t1", "type": "channels", "sorting": "",
             "channel_ids": ["beta10", "beta2", "muted", "old", "unknown"]},
            {"id": "dms", "team_id": "t1", "type": "direct_messages", "sorting": "recent",
             "channel_ids": ["dm-bob", "dm-carol", "dm-dave", "dm-erin", "dm-me", "gm"]}
        ],
        "order": ["fav", "chan", "dms"]
    }))
    .unwrap();
    store.set_categories("t1".into(), categories);
    store
}

fn ids(view: &SidebarCategoryView) -> Vec<&str> {
    view.channels
        .iter()
        .map(|channel| channel.id.as_str())
        .collect()
}

#[test]
fn categories_filter_and_sort() {
    let store = store();
    let sidebar = store.sidebar(&"t1".into(), None);
    assert!(sidebar.unreads.is_none());
    let [fav, chan, dms] = sidebar.categories.as_slice() else {
        panic!("three categories");
    };
    assert_eq!(ids(fav), ["beta9", "alpha"], "manual order");
    // By name with numbers compared as numbers, muted last; archived and unknown left out.
    assert_eq!(ids(chan), ["beta2", "beta10", "muted"]);
    // Most recent first. carol: unread without a preference; dave and the self-DM: closed;
    // erin: deactivated after the last view.
    assert_eq!(ids(dms), ["gm", "dm-bob", "dm-carol"]);
    assert_eq!(
        dms.channels[0].name, "bob, carol",
        "GM names sorted, without me"
    );
}

#[test]
fn the_current_channel_is_always_shown() {
    let store = store();
    let sidebar = store.sidebar(&"t1".into(), Some(&"old".into()));
    assert!(ids(&sidebar.categories[1]).contains(&"old"));
    let sidebar = store.sidebar(&"t1".into(), Some(&"dm-erin".into()));
    let dms = &sidebar.categories[2];
    let erin = dms
        .channels
        .iter()
        .find(|channel| channel.id.as_str() == "dm-erin")
        .unwrap();
    assert!(erin.current && erin.shown_when_collapsed());
}

#[test]
fn the_dm_limit_keeps_unread_and_recently_viewed() {
    let mut store = store();
    store.preferences.insert(
        ("sidebar_settings".into(), "limit_visible_dms_gms".into()),
        "2".into(),
    );
    store
        .channel_members
        .get_mut(&ChannelId::from("dm-bob"))
        .unwrap()
        .last_viewed_at = 500;
    let sidebar = store.sidebar(&"t1".into(), None);
    // Unread carol first, then the most recently viewed (bob); the GM doesn't fit.
    assert_eq!(ids(&sidebar.categories[2]), ["dm-bob", "dm-carol"]);
}

#[test]
fn the_unreads_group() {
    let mut store = store();
    store.preferences.insert(
        ("sidebar_settings".into(), "show_unread_section".into()),
        "true".into(),
    );
    // alpha gets a mention, beta10 a plain unread.
    for (id, mentions) in [("alpha", 1), ("beta10", 0)] {
        let member = store.channel_members.get_mut(&ChannelId::from(id)).unwrap();
        member.msg_count = 0;
        member.msg_count_root = 0;
        member.mention_count = mentions;
        member.mention_count_root = mentions;
    }
    let sidebar = store.sidebar(&"t1".into(), None);
    let unreads: Vec<&str> = sidebar
        .unreads
        .as_ref()
        .unwrap()
        .iter()
        .map(|channel| channel.id.as_str())
        .collect();
    // Mentions first, then the most recent post.
    assert_eq!(unreads, ["alpha", "dm-carol", "beta10"]);
    assert_eq!(ids(&sidebar.categories[0]), ["beta9"]);
}

#[test]
fn unread_dms_without_a_preference_get_one() {
    let store = store();
    let preferences = store.dm_gm_show_preferences_to_save();
    assert_eq!(preferences.len(), 1);
    assert_eq!(
        (
            preferences[0].category.as_str(),
            preferences[0].name.as_str()
        ),
        ("direct_channel_show", "carol")
    );
}

#[test]
fn numeric_name_order() {
    let mut names = vec!["b 10", "B 9", "a", "b 9x", "b"];
    names.sort_by(|a, b| compare_names(a, b));
    assert_eq!(names, ["a", "b", "B 9", "b 9x", "b 10"]);
}
