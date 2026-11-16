// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn a_new_picture_replaces_the_old_ones() {
    let dir = tempfile::tempdir().unwrap();
    let old = ImageKey::Avatar {
        user: UserId::new("u1"),
        updated: 1,
    };
    let new = ImageKey::Avatar {
        user: UserId::new("u1"),
        updated: 2,
    };
    let other = ImageKey::Avatar {
        user: UserId::new("u2"),
        updated: 1,
    };
    save(dir.path(), &old, b"old").unwrap();
    save(dir.path(), &other, b"other").unwrap();
    save(dir.path(), &new, b"new").unwrap();
    assert!(!old.path(dir.path()).exists());
    assert_eq!(std::fs::read(new.path(dir.path())).unwrap(), b"new");
    assert!(other.path(dir.path()).exists());

    let emoji = ImageKey::Emoji(EmojiId::new("e1"));
    save(dir.path(), &emoji, b"gif").unwrap();
    assert!(emoji.path(dir.path()).ends_with("images/emoji/e1"));
}
