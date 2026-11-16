// SPDX-License-Identifier: AGPL-3.0-only

//! MatterLite's Warp-style window shell (MatterLite.md §10.2): chat tab bar, tool panel with a
//! toolbelt, tab content and right panel. Panes are placeholders until the features land.

mod buttons;
mod right_panel;
pub mod search_bar;
mod tab_bar;
mod tool_panel;
mod workspace;

pub use search_bar::{
    Matcher, SearchBarEvent, SearchBarOptions, SearchBarView, ToggleGroup, ToggleSpec,
};
pub use tab_bar::{AccountBadge, ConnectionStatus, TabBadge, TabLeading, TabMenuItem, TabSpec};
pub use tool_panel::{ToolPane, ToolPanel, ToolPanelTab, ViewPane};
pub use workspace::{Workspace, WorkspaceAction, WorkspaceEvent, init};

/// Height of the tab bar, without its 1-px bottom border.
pub const TAB_BAR_HEIGHT: f32 = 34.;
/// Height of the tool panel and right panel headers; matches the tab bar.
pub const PANEL_HEADER_HEIGHT: f32 = TAB_BAR_HEIGHT;
