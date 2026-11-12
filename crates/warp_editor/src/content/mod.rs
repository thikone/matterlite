// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/editor/src/content/mod.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

pub mod anchor;
pub mod buffer;
pub mod core;
mod cursor;
pub mod diff;
pub mod edit;
pub mod find;
pub mod hidden_lines_model;
pub mod markdown;
pub mod mermaid_diagram;
pub mod outline;
mod segmentation;
pub mod selection;
pub mod selection_model;
pub mod text;
pub mod undo;
mod validation;
pub mod version;
