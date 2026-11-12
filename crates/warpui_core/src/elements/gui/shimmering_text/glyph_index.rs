// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/elements/gui/shimmering_text/glyph_index.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct GlyphIndex<T>(pub(super) T);
