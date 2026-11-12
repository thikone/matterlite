// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/vim/src/register.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

/// In Vim, see ":help quote_".
/// The black hole register can be written to but it always reads as empty.
pub const BLACK_HOLE_REGISTER: char = '_';

/// The registers we currently support.
pub fn valid_register_name(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '+' | '*' | '"')
}
