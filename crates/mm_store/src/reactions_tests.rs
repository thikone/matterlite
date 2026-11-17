// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;
use std::sync::Arc;

use mm_model::{ClientConfig, Post, PostMetadata, User};

use super::*;

fn store() -> Store {
    Store::new(User {
        id: "me".into(),
        username: "alice".into(),
        ..User::default()
    })
}

fn set_recent(store: &mut Store, value: &str) {
    store.put_preferences(&[Preference {
        user_id: "me".into(),
        category: RECENT_EMOJIS.into(),
        name: "me".into(),
        value: value.into(),
    }]);
}

#[test]
fn recent_emojis_count_uses_as_the_webapp_does() {
    let mut store = store();
    assert!(store.recent_emojis().is_empty());
    // Stored least used first; plain names from older clients count as used once.
    set_recent(
        &mut store,
        r#"["smile", {"name": "tada", "usageCount": 1}, {"name": "+1", "usageCount": 3}]"#,
    );
    assert_eq!(store.recent_emojis(), ["+1", "tada", "smile"]);

    let preference = store.recent_emojis_after("smile");
    assert_eq!(
        (preference.category.as_str(), preference.name.as_str()),
        (RECENT_EMOJIS, "me")
    );
    let value: serde_json::Value = serde_json::from_str(&preference.value).unwrap();
    assert_eq!(
        value,
        serde_json::json!([
            {"name": "tada", "usageCount": 1},
            {"name": "smile", "usageCount": 2},
            {"name": "+1", "usageCount": 3},
        ])
    );
    set_recent(&mut store, &preference.value);
    let preference = store.recent_emojis_after("wave");
    set_recent(&mut store, &preference.value);
    // A new emoji goes after the others used once.
    assert_eq!(store.recent_emojis(), ["+1", "smile", "wave", "tada"]);
}

#[test]
fn recent_emojis_keep_the_newest_27() {
    let mut store = store();
    for index in 0..30 {
        let preference = store.recent_emojis_after(&format!("e{index}"));
        set_recent(&mut store, &preference.value);
    }
    let recent = store.recent_emojis();
    assert_eq!(recent.len(), MAX_RECENT_EMOJIS);
    assert_eq!(recent.first().map(String::as_str), Some("e29"));
    assert_eq!(recent.last().map(String::as_str), Some("e3"));
}

#[test]
fn the_reaction_limit_counts_distinct_emoji() {
    let mut store = store();
    let reaction = |user: &str, name: &str| Reaction {
        user_id: user.into(),
        post_id: "p".into(),
        emoji_name: name.into(),
        ..Reaction::default()
    };
    store.posts.insert(
        "p".into(),
        Arc::new(Post {
            id: "p".into(),
            metadata: Some(PostMetadata {
                reactions: vec![reaction("bob", "smile"), reaction("carol", "smile")],
                ..PostMetadata::default()
            }),
            ..Post::default()
        }),
    );
    let post = PostId::from("p");
    // No limit without the setting.
    assert!(store.reaction_allowed(&post, "tada"));
    store.set_config(ClientConfig(HashMap::from([(
        "UniqueEmojiReactionLimitPerPost".to_owned(),
        "2".to_owned(),
    )])));
    assert!(store.reaction_allowed(&post, "tada"));
    store.put_reaction(&reaction("me", "tada"), true);
    assert!(!store.reaction_allowed(&post, "wave"));
    // Adding to an emoji that is there always works.
    assert!(store.reaction_allowed(&post, "smile"));

    // The server's confirmation and its event change the post once.
    store.put_reaction(&reaction("me", "tada"), true);
    let reactions = &store.posts[&post].metadata.as_ref().unwrap().reactions;
    assert_eq!(reactions.len(), 3);
    store.put_reaction(&reaction("me", "tada"), false);
    assert!(store.reaction_allowed(&post, "wave"));
}

#[test]
fn skin_tone_default_is_none() {
    let mut store = store();
    assert_eq!(store.skin_tone(), None);
    let tone = |value: &str| Preference {
        user_id: "me".into(),
        category: "emoji".into(),
        name: "emoji_skintone".into(),
        value: value.into(),
    };
    store.put_preferences(&[tone("default")]);
    assert_eq!(store.skin_tone(), None);
    store.put_preferences(&[tone("1F3FD")]);
    assert_eq!(store.skin_tone(), Some("1F3FD"));
}
