// SPDX-License-Identifier: AGPL-3.0-only

//! Themes for MatterLite: Warp's built-in themes, custom YAML themes, the theme settings, the
//! [`AppearanceManager`] that applies them and the theme chooser. Mattermost themes and
//! `ChatColors` come later (MatterLite.md §11).

pub mod appearance;
pub mod custom_themes;
pub mod default_themes;
pub mod preview;
pub mod settings;
pub mod theme;
pub mod theme_chooser;

pub use appearance::{Appearance, AppearanceEvent, AppearanceManager, AppearanceManagerEvent};
pub use settings::ThemeSettings;
pub use theme::{ThemeKind, WarpThemeConfig};
pub use theme_chooser::{ThemeChooser, ThemeChooserEvent, ThemeChooserMode};
pub use warp_core::ui::theme::WarpTheme;
