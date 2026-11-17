// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn labels(actions: PostActions, replies: i64) -> Vec<Vec<&'static str>> {
    PostMenu::new(&Post::default(), None, false, actions, replies)
        .items()
        .into_iter()
        .map(|group| group.into_iter().map(|(_, label)| label).collect())
        .collect()
}

#[test]
fn entries_follow_the_actions_in_the_webapp_order() {
    let all = PostActions {
        reply: true,
        react: true,
        unreact: true,
        emoji_picker: true,
        follow: true,
        following: false,
        mark_unread: true,
        save: true,
        saved: false,
        pin: true,
        pinned: false,
        copy: true,
        edit: true,
        delete: true,
    };
    assert_eq!(
        labels(all, 3),
        [
            vec![
                "Reply",
                "Follow thread",
                "Mark as unread",
                "Save",
                "Pin to channel"
            ],
            vec!["Copy link", "Copy text"],
            vec!["Edit", "Delete"],
        ]
    );
    let toggled = PostActions {
        following: true,
        saved: true,
        pinned: true,
        ..all
    };
    assert_eq!(
        labels(toggled, 0)[0],
        [
            "Reply",
            "Unfollow message",
            "Mark as unread",
            "Remove from saved",
            "Unpin from channel"
        ]
    );
}

#[test]
fn empty_groups_are_left_out() {
    let archived = PostActions {
        save: true,
        copy: true,
        ..PostActions::default()
    };
    assert_eq!(
        labels(archived, 0),
        [vec!["Save"], vec!["Copy link", "Copy text"]]
    );
}
