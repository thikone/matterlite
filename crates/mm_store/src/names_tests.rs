// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn user(username: &str, first: &str, last: &str, nickname: &str) -> User {
    User {
        username: username.into(),
        first_name: first.into(),
        last_name: last.into(),
        nickname: nickname.into(),
        ..User::default()
    }
}

#[test]
fn name_formats() {
    let alice = user("alice", "Alice", "Anderson", "Ali");
    assert_eq!(NameFormat::Username.display(&alice), "alice");
    assert_eq!(NameFormat::FullName.display(&alice), "Alice Anderson");
    assert_eq!(NameFormat::NicknameFullName.display(&alice), "Ali");

    let bare = user("bob", "", "", "");
    for format in [
        NameFormat::Username,
        NameFormat::FullName,
        NameFormat::NicknameFullName,
    ] {
        assert_eq!(format.display(&bare), "bob");
    }
    assert_eq!(
        NameFormat::NicknameFullName.display(&user("c", "", "Chen", "")),
        "Chen"
    );
    assert_eq!(NameFormat::parse("full_name"), NameFormat::FullName);
    assert_eq!(NameFormat::parse(""), NameFormat::Username);
}

#[test]
fn dm_partners() {
    let me = UserId::new("aaa");
    assert_eq!(dm_partner("aaa__bbb", &me), Some(UserId::new("bbb")));
    assert_eq!(dm_partner("bbb__aaa", &me), Some(UserId::new("bbb")));
    assert_eq!(dm_partner("aaa__aaa", &me), Some(UserId::new("aaa")));
    assert_eq!(dm_partner("town-square", &me), None);
}
