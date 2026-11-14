// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn fixture() -> PostList {
    serde_json::from_str(include_str!("../fixtures/post_list.json")).unwrap()
}

#[test]
fn post_list_reads_order_and_posts() {
    let list = fixture();
    let ids: Vec<&str> = list.ordered().map(|post| post.id.as_str()).collect();
    assert_eq!(
        ids,
        ["w7ab3b8k6tgmdmbrc6s8xy1n4c", "k9q6fxm3u7rdmrj7d1mk6c5wzr"]
    );
    assert!(list.next_post_id.is_empty());
    assert_eq!(list.prev_post_id.as_str(), "p1o2i3u4y5t6r7e8w9q0a1s2d3");
    assert_eq!(list.has_next, None);
}

#[test]
fn reply_and_root_fields() {
    let list = fixture();
    let reply = &list.posts["w7ab3b8k6tgmdmbrc6s8xy1n4c"];
    assert!(!reply.is_root());
    assert!(
        reply.participants.is_empty(),
        "null participants read as empty"
    );
    assert!(reply.file_ids.is_empty(), "missing file_ids read as empty");
    let reactions = &reply.metadata.as_ref().unwrap().reactions;
    assert_eq!(reactions[0].emoji_name, "tada");

    let root = &list.posts["k9q6fxm3u7rdmrj7d1mk6c5wzr"];
    assert!(root.is_root());
    assert!(root.is_pinned);
    assert_eq!(root.is_following, Some(true));
    assert_eq!(root.participants[0].username, "bob");
    assert!(root.participants[0].timezone.is_empty());
    let metadata = root.metadata.as_ref().unwrap();
    assert_eq!(metadata.files[0].name, "notes.pdf");
    assert_eq!(metadata.files[0].mini_preview, None);
    assert_eq!(metadata.embeds.len(), 1);
}

#[test]
fn post_round_trips() {
    let list = fixture();
    let text = serde_json::to_string(&list).unwrap();
    let again: PostList = serde_json::from_str(&text).unwrap();
    assert_eq!(again, list);
}

#[test]
fn system_and_deleted_posts() {
    let post: Post = serde_json::from_str(
        r#"{"id": "x", "type": "system_join_channel", "delete_at": 5, "props": null}"#,
    )
    .unwrap();
    assert!(post.is_system());
    assert!(post.is_deleted());
    assert!(post.props.is_empty());
}
