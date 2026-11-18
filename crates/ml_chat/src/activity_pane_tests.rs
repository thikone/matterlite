// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::{Channel, ChannelMember, Preference, User};

use super::*;

#[test]
fn excerpts_are_one_line_and_cut() {
    assert_eq!(excerpt("Ship\n\n  it   today"), "Ship it today");
    let long = "word ".repeat(60);
    let cut = excerpt(&long);
    assert!(cut.ends_with('…'));
    assert!(cut.chars().count() <= EXCERPT + 1);
}

#[test]
fn direct_messages_belong_to_every_team() {
    let mut store = Store::new(User {
        id: "me".into(),
        ..User::default()
    });
    let channel = |id: &str, team: &str| Channel {
        id: id.into(),
        team_id: team.into(),
        ..Channel::default()
    };
    let member = |id: &str| ChannelMember {
        channel_id: id.into(),
        user_id: "me".into(),
        ..ChannelMember::default()
    };
    store.set_channels(
        vec![channel("c1", "t1"), channel("dm", "")],
        vec![member("c1"), member("dm")],
    );
    let (t1, t2) = (TeamId::from("t1"), TeamId::from("t2"));
    assert!(in_team(&store, &"c1".into(), Some(&t1)));
    assert!(!in_team(&store, &"c1".into(), Some(&t2)));
    assert!(in_team(&store, &"dm".into(), Some(&t2)));
    assert!(!in_team(&store, &"gone".into(), Some(&t1)));

    store.put_preferences(&[
        Preference {
            user_id: "me".into(),
            category: "flagged_post".into(),
            name: "p1".into(),
            value: "true".into(),
        },
        Preference {
            user_id: "me".into(),
            category: "flagged_post".into(),
            name: "p2".into(),
            value: "false".into(),
        },
    ]);
    assert_eq!(saved_ids(&store), HashSet::from(["p1".to_owned()]));
}
