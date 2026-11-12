// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/keymap/context_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use super::macros::*;
use super::*;

#[test]
fn test_context_predicate_eval() -> anyhow::Result<()> {
    let predicate = id!("a") & id!("b") | eq!("c", "d");

    let mut context = Context::default();
    context.set.insert("a");
    assert!(!predicate.eval(&context));

    context.set.insert("b");
    assert!(predicate.eval(&context));

    context.set.remove("b");
    context.map.insert("c", "x");
    assert!(!predicate.eval(&context));

    context.map.insert("c", "d");
    assert!(predicate.eval(&context));

    let predicate = !id!("a");
    assert!(predicate.eval(&Context::default()));

    Ok(())
}
