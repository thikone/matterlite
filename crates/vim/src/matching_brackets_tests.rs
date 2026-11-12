// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/vim/src/matching_brackets_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use super::*;

impl BracketChar {
    fn from_char(c: char) -> Self {
        Self::try_from(c).unwrap_or_else(|_| panic!("invalid bracket char: {c}"))
    }
}

#[test]
fn test_vim_find_matching_bracket() {
    assert_eq!(
        vim_find_matching_bracket("", &BracketChar::from_char('('), 0),
        None,
    );
    assert_eq!(
        vim_find_matching_bracket("foo(bar)baz", &BracketChar::from_char('('), 3),
        Some(7.into())
    );
    assert_eq!(
        vim_find_matching_bracket("foo(bar)baz", &BracketChar::from_char(')'), 7),
        Some(3.into())
    );
    assert_eq!(
        vim_find_matching_bracket("foo(bar)baz", &BracketChar::from_char('('), 8),
        None
    );
    assert_eq!(
        vim_find_matching_bracket("foo[bar]baz", &BracketChar::from_char('('), 3),
        None
    );
    assert_eq!(
        vim_find_matching_bracket("foo(bar(hello) world)baz", &BracketChar::from_char('('), 3),
        Some(20.into())
    );
    assert_eq!(
        vim_find_matching_bracket("foo(bar(hello) world)baz", &BracketChar::from_char(')'), 20),
        Some(3.into())
    );
    assert_eq!(
        vim_find_matching_bracket(
            "foo(bar(h[(])llo) world)baz",
            &BracketChar::from_char('('),
            3
        ),
        Some(23.into())
    );
    assert_eq!(
        vim_find_matching_bracket(
            "function foo() {\necho hello world\necho hi\n}\nfoo",
            &BracketChar::from_char('{'),
            15
        ),
        Some(42.into())
    );
    assert_eq!(
        vim_find_matching_bracket(
            "function foo() {\necho hello world\necho hi\n}\nfoo",
            &BracketChar::from_char('}'),
            42
        ),
        Some(15.into())
    );
}
