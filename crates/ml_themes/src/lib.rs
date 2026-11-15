// SPDX-License-Identifier: AGPL-3.0-only

//! Themes for MatterLite: Warp's built-in themes, custom YAML themes, the theme settings, the
//! [`AppearanceManager`] that applies them and the theme chooser, the Mattermost themes and the
//! signed-in account's theme (MatterLite.md §11). Every theme carries a chat palette:
//! `theme.chat()` ([`ChatColors`]).

pub mod appearance;
pub mod custom_themes;
pub mod default_themes;
pub mod mattermost;
pub mod preview;
pub mod settings;
pub mod theme;
pub mod theme_chooser;

pub use appearance::{Appearance, AppearanceEvent, AppearanceManager, AppearanceManagerEvent};
pub use pathfinder_color::ColorU;
pub use settings::ThemeSettings;
pub use theme::{ThemeKind, WarpThemeConfig};
pub use theme_chooser::{ThemeChooser, ThemeChooserEvent, ThemeChooserMode};
pub use warp_core::ui::theme::WarpTheme;
pub use warp_core::ui::theme::chat::{ChatColorOverrides, ChatColors};
