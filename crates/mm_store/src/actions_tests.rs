// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;
use std::sync::Arc;

use mm_model::{Channel, ChannelMember, ClientConfig, Post, Preference, Role, User};

use super::*;
use crate::PendingPost;

const NOW: i64 = 1_000_000_000;

fn config(pairs: &[(&str, &str)]) -> ClientConfig {
    ClientConfig(HashMap::from_iter(
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())),
    ))
}

fn store() -> Store {
    let mut store = Store::new(User {
        id: "me".into(),
        username: "alice".into(),
        ..User::default()
    });
    store.set_channels(
        vec![Channel {
            id: "c1".into(),
            team_id: "t1".into(),
            name: "town-square".into(),
            ..Channel::default()
        }],
        vec![ChannelMember {
            channel_id: "c1".into(),
            user_id: "me".into(),
            roles: "channel_user".into(),
            ..ChannelMember::default()
        }],
    );
    store.set_roles(vec![
        Role {
            name: "channel_user".into(),
            permissions: ["create_post", "add_reaction", "edit_post", "delete_post"]
                .map(String::from)
                .to_vec(),
            ..Role::default()
        },
        Role {
            name: "channel_admin".into(),
            permissions: ["edit_others_posts", "delete_others_posts"]
                .map(String::from)
                .to_vec(),
            ..Role::default()
        },
    ]);
    for (id, user) in [("mine", "me"), ("theirs", "bob")] {
        store.posts.insert(
            id.into(),
            Arc::new(Post {
                id: id.into(),
                user_id: user.into(),
                channel_id: "c1".into(),
                create_at: NOW - 60_000,
                message: "hi".into(),
                ..Post::default()
            }),
        );
    }
    store
}

#[test]
fn my_post_can_be_edited_and_deleted_but_not_others() {
    let store = store();
    let mine = store.post_actions(&"mine".into(), NOW);
    assert!(mine.reply && mine.react && mine.edit && mine.delete && mine.copy);
    assert!(mine.pin && mine.save && mine.mark_unread);
    assert!(!mine.follow, "CRT is off");
    let theirs = store.post_actions(&"theirs".into(), NOW);
    assert!(theirs.reply && !theirs.edit && !theirs.delete);
}

#[test]
fn a_channel_admin_edits_and_deletes_others_posts() {
    let mut store = store();
    store.channel_members.get_mut("c1").unwrap().roles = "channel_user channel_admin".into();
    let theirs = store.post_actions(&"theirs".into(), NOW);
    assert!(theirs.edit && theirs.delete);
}

#[test]
fn the_edit_time_limit_needs_a_licence() {
    let mut store = store();
    store.set_config(config(&[("PostEditTimeLimit", "30")]));
    assert!(store.post_actions(&"mine".into(), NOW).edit, "unlicensed");
    store.set_license(config(&[("IsLicensed", "true")]));
    assert!(!store.post_actions(&"mine".into(), NOW).edit, "60 s old");
    assert!(
        store.post_actions(&"mine".into(), NOW - 40_000).edit,
        "20 s old"
    );
    store.set_config(config(&[("PostEditTimeLimit", "-1")]));
    assert!(store.post_actions(&"mine".into(), NOW).edit, "no limit");
}

#[test]
fn an_archived_channel_only_copies_and_saves() {
    let mut store = store();
    Arc::make_mut(store.channels.get_mut("c1").unwrap()).delete_at = 1;
    let actions = store.post_actions(&"mine".into(), NOW);
    assert!(actions.copy && actions.save);
    assert!(!actions.reply && !actions.react && !actions.edit && !actions.delete);
    assert!(!actions.pin && !actions.mark_unread);
}

#[test]
fn system_pending_and_deleted_posts_have_no_actions() {
    let mut store = store();
    let mut post = (*store.posts[&PostId::from("mine")]).clone();
    post.post_type = "system_join_channel".into();
    store.posts.insert("system".into(), Arc::new(post.clone()));
    post.post_type.clear();
    post.delete_at = 1;
    store.posts.insert("deleted".into(), Arc::new(post.clone()));
    post.delete_at = 0;
    post.pending_post_id = "me:1".into();
    store.posts.insert("me:1".into(), Arc::new(post));
    store.pending.insert(
        "me:1".into(),
        PendingPost {
            channel: "c1".into(),
            root: PostId::default(),
            failed: false,
        },
    );
    for id in ["system", "deleted", "me:1", "unknown"] {
        assert!(!store.post_actions(&id.into(), NOW).any(), "{id}");
    }
}

#[test]
fn saved_pinned_and_followed_state() {
    let mut store = store();
    store.set_config(config(&[("CollapsedThreads", "always_on")]));
    store.set_preferences(vec![Preference {
        user_id: "me".into(),
        category: keys::FLAGGED_POST.into(),
        name: "mine".into(),
        value: "true".into(),
    }]);
    Arc::make_mut(store.posts.get_mut("mine").unwrap()).is_pinned = true;
    Arc::make_mut(store.posts.get_mut("theirs").unwrap()).is_following = Some(true);
    let mine = store.post_actions(&"mine".into(), NOW);
    assert!(mine.saved && mine.pinned && mine.follow && !mine.following);
    let theirs = store.post_actions(&"theirs".into(), NOW);
    assert!(!theirs.saved && !theirs.pinned && theirs.following);

    // A reply follows its root.
    let mut reply = (*store.posts[&PostId::from("mine")]).clone();
    reply.id = "reply".into();
    reply.root_id = "theirs".into();
    store.posts.insert("reply".into(), Arc::new(reply));
    assert!(store.post_actions(&"reply".into(), NOW).following);
}
