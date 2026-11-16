// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

const PLAIN: EnterKey = EnterKey::Plain;
const CTRL: EnterKey = EnterKey::CtrlOrCmd;

fn send(text: &str) -> OnEnter {
    OnEnter::Send(text.to_owned())
}

#[test]
fn enter_sends_except_inside_an_open_code_block() {
    let prefs = EnterPrefs::default();
    assert_eq!(on_enter("hello", 5, PLAIN, prefs), send("hello"));
    let open = "```rust\nfn main() {}";
    assert_eq!(on_enter(open, open.len(), PLAIN, prefs), OnEnter::NewLine);
    // Ctrl/Cmd+Enter sends from inside the block, closing it.
    assert_eq!(
        on_enter(open, open.len(), CTRL, prefs),
        send("```rust\nfn main() {}\n```")
    );
    // A closed block is no reason to wait.
    let closed = "```\ncode\n```";
    assert_eq!(on_enter(closed, closed.len(), PLAIN, prefs), send(closed));
    // The caret decides: before the fence, Enter sends.
    assert_eq!(on_enter(open, 0, PLAIN, prefs), send(open));
    // An empty block with only the fence doesn't get a closing one.
    assert_eq!(on_enter("```", 3, CTRL, prefs), send("```"));
}

#[test]
fn send_on_ctrl_enter() {
    let prefs = EnterPrefs {
        send_on_ctrl_enter: true,
        code_block_ctrl_enter: true,
    };
    assert_eq!(on_enter("hi", 2, PLAIN, prefs), OnEnter::NewLine);
    assert_eq!(on_enter("hi", 2, CTRL, prefs), send("hi"));
    let off = EnterPrefs {
        send_on_ctrl_enter: false,
        code_block_ctrl_enter: false,
    };
    // Both off: Enter always sends, even in a block.
    assert_eq!(on_enter("```\nx", 5, PLAIN, off), send("```\nx"));
}

#[test]
fn code_blocks_and_carets() {
    assert!(within_code_block("```\nx", 5));
    assert!(!within_code_block("```\nx\n```\n", 10));
    assert!(within_code_block("```a\n```\n```b\nz", 15));
    // Fences count only at the start of a line; a caret inside a character is safe.
    assert!(!within_code_block("say ```x", 8));
    assert!(within_code_block("```\né", 5));
}

#[test]
fn length_limit() {
    assert_eq!(length_error("abc", 3), None);
    assert_eq!(
        length_error("ééé€", 3).as_deref(),
        Some("Your message is too long. Character count: 4/3")
    );
}

#[test]
fn typing_users_and_text() {
    let start = Instant::now();
    let interval = Duration::from_secs(5);
    let mut typing = TypingUsers::default();
    typing.add("c".into(), PostId::default(), "bob".into(), start);
    typing.add("c".into(), PostId::default(), "carol".into(), start);
    typing.add("c".into(), "root".into(), "dave".into(), start);
    // Typing again refreshes, keeping one entry.
    typing.add(
        "c".into(),
        PostId::default(),
        "bob".into(),
        start + Duration::from_secs(3),
    );
    assert_eq!(
        typing.users(&"c".into(), &PostId::default()),
        [UserId::new("carol"), UserId::new("bob")]
    );
    assert!(typing.expire(start + Duration::from_secs(6), interval));
    assert_eq!(
        typing.users(&"c".into(), &PostId::default()),
        [UserId::new("bob")]
    );
    assert!(typing.posted(&"c".into(), &"bob".into()));
    assert!(typing.is_empty());

    assert_eq!(typing_text(&[]), None);
    assert_eq!(
        typing_text(&["carol".into()]).as_deref(),
        Some("carol is typing...")
    );
    assert_eq!(
        typing_text(&["bob".into(), "carol".into(), "dave".into()]).as_deref(),
        Some("bob, carol and dave are typing...")
    );

    let mut throttle = TypingThrottle::default();
    assert!(throttle.due(start, interval));
    assert!(!throttle.due(start + Duration::from_secs(4), interval));
    assert!(throttle.due(start + Duration::from_secs(5), interval));
    throttle.reset();
    assert!(throttle.due(start + Duration::from_secs(6), interval));
}
