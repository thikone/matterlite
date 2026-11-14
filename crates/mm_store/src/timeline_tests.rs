// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn key(at: i64) -> Key {
    (at, PostId::new(format!("p{at:03}")))
}

fn keys(range: std::ops::RangeInclusive<i64>) -> Vec<Key> {
    range.map(key).collect()
}

fn shown(timeline: &Timeline) -> Vec<Option<i64>> {
    timeline
        .items()
        .into_iter()
        .map(|id| id.map(|id| timeline.key_of(id).unwrap().0))
        .collect()
}

#[test]
fn newest_page_then_older_pages_merge() {
    let mut timeline = Timeline::new();
    timeline.insert_page(Page {
        keys: keys(7..=9),
        reaches_newest: true,
        ..Page::default()
    });
    assert!(timeline.has_recent());
    assert_eq!(shown(&timeline), [Some(9), Some(8), Some(7), None]);
    assert_eq!(timeline.oldest_of_recent(), Some(&key(7)));

    // `before=p007`: the anchor joins the page to the range.
    timeline.insert_page(Page {
        keys: keys(4..=6),
        adjacent: vec![key(7)],
        ..Page::default()
    });
    assert_eq!(timeline.ranges().len(), 1);
    timeline.insert_page(Page {
        keys: keys(1..=3),
        adjacent: vec![key(4)],
        reaches_start: true,
        ..Page::default()
    });
    assert!(timeline.is_complete());
    assert_eq!(
        shown(&timeline),
        (1..=9).rev().map(Some).collect::<Vec<_>>()
    );
}

#[test]
fn a_newer_page_without_overlap_leaves_a_hole() {
    let mut timeline = Timeline::new();
    timeline.insert_page(Page {
        keys: keys(1..=3),
        reaches_newest: true,
        reaches_start: true,
        ..Page::default()
    });
    // After a long offline time the `since` sync hit the cap: the newest page is loaded on
    // its own and doesn't reach the old posts.
    timeline.insert_page(Page {
        keys: keys(20..=22),
        reaches_newest: true,
        ..Page::default()
    });
    assert_eq!(timeline.ranges().len(), 2);
    assert!(!timeline.ranges()[0].recent && timeline.ranges()[1].recent);
    assert_eq!(timeline.holes(), [(key(20), key(3))]);
    assert_eq!(
        shown(&timeline),
        [
            Some(22),
            Some(21),
            Some(20),
            None,
            Some(3),
            Some(2),
            Some(1)
        ]
    );

    // Backfill from the newer side closes it.
    timeline.insert_page(Page {
        keys: keys(4..=19),
        // `before=p020`; the server names p003 as `prev_post_id`.
        adjacent: vec![key(20), key(3)],
        ..Page::default()
    });
    assert!(timeline.is_complete());
    assert!(timeline.holes().is_empty());
}

#[test]
fn live_posts_join_only_a_recent_range() {
    let mut timeline = Timeline::new();
    assert!(!timeline.insert_live(key(5)), "never loaded");
    assert_eq!(timeline.post_ids().count(), 0);

    timeline.insert_page(Page {
        keys: keys(1..=3),
        reaches_newest: true,
        ..Page::default()
    });
    assert!(timeline.insert_live(key(6)));
    assert_eq!(timeline.newest(), Some(&key(6)));
    // p005 arrived before the page was loaded and is inside the range now.
    assert_eq!(shown(&timeline)[..4], [Some(6), Some(5), Some(3), Some(2)]);
}

#[test]
fn empty_channel_accepts_live_posts() {
    let mut timeline = Timeline::new();
    timeline.insert_page(Page {
        reaches_newest: true,
        reaches_start: true,
        ..Page::default()
    });
    assert!(timeline.is_complete() && timeline.has_recent());
    assert!(timeline.insert_live(key(1)));
    assert!(timeline.is_complete());
    assert_eq!(shown(&timeline), [Some(1)]);
}

#[test]
fn remove_and_rekey() {
    let mut timeline = Timeline::new();
    timeline.insert_page(Page {
        keys: keys(1..=3),
        reaches_newest: true,
        reaches_start: true,
        ..Page::default()
    });
    let pending = (10, PostId::new("me:10"));
    timeline.insert_live(pending.clone());
    timeline.rekey(&pending.1, 12);
    assert_eq!(timeline.key_of(&pending.1), Some((12, pending.1.clone())));
    assert_eq!(timeline.len(), 4);

    assert!(timeline.remove(&PostId::new("p002")));
    assert!(!timeline.remove(&PostId::new("p002")));
    assert_eq!(shown(&timeline), [Some(12), Some(3), Some(1)]);
}

#[test]
fn reinserting_a_post_with_a_new_time_moves_it() {
    let mut timeline = Timeline::new();
    timeline.insert_page(Page {
        keys: vec![key(1), key(2)],
        reaches_newest: true,
        reaches_start: true,
        ..Page::default()
    });
    timeline.insert_page(Page {
        keys: vec![(5, PostId::new("p001"))],
        reaches_newest: true,
        adjacent: vec![key(2)],
        ..Page::default()
    });
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline.newest(), Some(&(5, PostId::new("p001"))));
}
