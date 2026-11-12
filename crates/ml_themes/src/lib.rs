// SPDX-License-Identifier: AGPL-3.0-only

//! Themes for MatterLite: Warp's built-in themes, the theme settings and the
//! [`AppearanceManager`] that applies them. Mattermost themes and `ChatColors` come later
//! (MatterLite.md §11).

pub mod appearance;
pub mod default_themes;
pub mod settings;
pub mod theme;

pub use appearance::{Appearance, AppearanceEvent, AppearanceManager};
pub use settings::ThemeSettings;
pub use theme::{ThemeKind, WarpThemeConfig};
pub use warp_core::ui::theme::WarpTheme;
