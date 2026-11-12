// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/editor/src/decoration/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use warpui_core::ModelContext;

use crate::content::buffer::BufferSnapshot;
use crate::content::edit::PreciseDelta;
use crate::content::version::BufferVersion;

pub trait DecorationLayer {
    fn update_internal_state_with_delta(
        &mut self,
        deltas: &[PreciseDelta],
        content_version: BufferVersion,
        content: BufferSnapshot,
        ctx: &mut ModelContext<Self>,
    );
}
