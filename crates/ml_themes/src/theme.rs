// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/themes/theme.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use std::collections::HashMap;
use std::iter::FromIterator;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use warp_core::ui::color::blend::Blend;
pub use warp_core::ui::theme::*;

use crate::default_themes::*;

// We use the discriminant of enum variants to determine the order of theme types in the
// theme chooser view.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    Hash,
    Eq,
    Ord,
    PartialOrd,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "The color theme.", rename_all = "snake_case")]
pub enum ThemeKind {
    #[schemars(description = "Adeberry")]
    Adeberry,
    #[schemars(description = "Phenomenon")]
    Phenomenon,
    #[default]
    #[schemars(description = "Dark")]
    Dark,
    #[schemars(description = "Dracula")]
    Dracula,
    #[schemars(description = "Fancy Dracula")]
    FancyDracula,
    #[schemars(description = "Cyber Wave")]
    CyberWave,
    #[schemars(description = "Solar Flare")]
    SolarFlare,
    #[schemars(description = "Solarized Dark")]
    SolarizedDark,
    #[schemars(description = "Willow Dream")]
    WillowDream,
    #[schemars(description = "Light")]
    Light,
    #[schemars(description = "Dark City")]
    DarkCity,
    #[schemars(description = "Gruvbox Dark")]
    GruvboxDark,
    #[schemars(description = "Red Rock")]
    RedRock,
    #[schemars(description = "Jellyfish")]
    JellyFish,
    #[schemars(description = "Leafy")]
    Leafy,
    #[schemars(description = "Koi")]
    Koi,
    #[schemars(description = "Solarized Light")]
    SolarizedLight,
    #[schemars(description = "Snowy")]
    Snowy,
    #[schemars(description = "Gruvbox Light")]
    GruvboxLight,
    #[schemars(description = "Pink City")]
    PinkCity,
    #[schemars(description = "Marble")]
    Marble,
    #[schemars(description = "A user-provided custom theme loaded from a file.")]
    Custom(CustomTheme),
    /// Base16 themes are a special case of custom themes with their own semantics for ANSI colors that override "bright" color variants.
    #[schemars(description = "A custom theme using the Base16 color scheme format.")]
    CustomBase16(CustomTheme),
}

impl From<CustomTheme> for ThemeKind {
    fn from(custom_theme: CustomTheme) -> ThemeKind {
        if custom_theme.name.as_str().starts_with("Base16") {
            ThemeKind::CustomBase16(custom_theme)
        } else {
            ThemeKind::Custom(custom_theme)
        }
    }
}

impl std::fmt::Display for ThemeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match &self {
            ThemeKind::Light => "Light",
            ThemeKind::Dark => "Dark",
            ThemeKind::Dracula => "Dracula",
            ThemeKind::SolarizedDark => "Solarized Dark",
            ThemeKind::SolarizedLight => "Solarized Light",
            ThemeKind::GruvboxDark => "Gruvbox Dark",
            ThemeKind::GruvboxLight => "Gruvbox Light",
            ThemeKind::JellyFish => "Jellyfish",
            ThemeKind::Koi => "Koi",
            ThemeKind::Leafy => "Leafy",
            ThemeKind::Marble => "Marble",
            ThemeKind::PinkCity => "Pink City",
            ThemeKind::Snowy => "Snowy",
            ThemeKind::DarkCity => "Dark City",
            ThemeKind::RedRock => "Red Rock",
            ThemeKind::CyberWave => "Cyber Wave",
            ThemeKind::WillowDream => "Willow Dream",
            ThemeKind::FancyDracula => "Fancy Dracula",
            ThemeKind::Phenomenon => "Phenomenon",
            ThemeKind::SolarFlare => "Solar Flare",
            ThemeKind::Adeberry => "Adeberry",
            ThemeKind::Custom(custom_theme) => custom_theme.name.as_str(),
            ThemeKind::CustomBase16(custom_theme) => custom_theme.name.as_str(),
        };
        write!(f, "{value}")
    }
}

impl ThemeKind {
    pub fn matches(&self, query: &str) -> bool {
        let theme_name = format!("{self}").to_lowercase();
        theme_name.contains(&query.to_lowercase())
    }
}

#[derive(
    Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord, schemars::JsonSchema,
)]
#[schemars(description = "A user-provided custom theme.")]
pub struct CustomTheme {
    #[schemars(description = "The display name of the custom theme.")]
    name: String,
    #[serde(
        deserialize_with = "deserialize_custom_theme_path",
        serialize_with = "serialize_custom_theme_path"
    )]
    #[schemars(description = "The file path to the custom theme definition.")]
    path: PathBuf,
}

impl settings_value::SettingsValue for CustomTheme {
    fn to_file_value(&self) -> serde_json::Value {
        serde_json::json!({
            "name": &self.name,
            "path": custom_theme_path_storage_value(&self.path, &warp_core::paths::themes_dir()),
        })
    }

    fn from_file_value(value: &serde_json::Value) -> Option<Self> {
        #[derive(Deserialize)]
        struct FileValue {
            name: String,
            path: String,
        }

        let value = serde_json::from_value::<FileValue>(value.clone()).ok()?;
        Some(Self {
            name: value.name,
            path: portable_custom_theme_path_from_stored_raw(
                &value.path,
                &warp_core::paths::themes_dir(),
            ),
        })
    }
}

fn serialize_custom_theme_path<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(path) = portable_custom_theme_storage_string(path, &warp_core::paths::themes_dir())
    {
        path.serialize(serializer)
    } else {
        path.serialize(serializer)
    }
}

fn deserialize_custom_theme_path<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let path = String::deserialize(deserializer)?;
    Ok(portable_custom_theme_path_from_stored_raw(
        &path,
        &warp_core::paths::themes_dir(),
    ))
}

fn custom_theme_path_storage_value(path: &Path, theme_root: &Path) -> serde_json::Value {
    if let Some(path) = portable_custom_theme_storage_string(path, theme_root) {
        serde_json::Value::String(path)
    } else {
        serde_json::json!(path)
    }
}

pub(crate) fn portable_custom_theme_path_from_stored_raw(raw: &str, theme_root: &Path) -> PathBuf {
    portable_stored_raw_components(raw)
        .map(|components| {
            components
                .iter()
                .fold(theme_root.to_path_buf(), |path, component| {
                    path.join(component)
                })
        })
        .unwrap_or_else(|| PathBuf::from(raw))
}

pub(crate) fn portable_custom_theme_storage_string(
    path: &Path,
    theme_root: &Path,
) -> Option<String> {
    if path_starts_with_windows_drive_prefix_using_forward_slash(path) {
        return None;
    }

    let relative = path.strip_prefix(theme_root).ok()?;
    let mut components = Vec::new();

    for component in relative.components() {
        let Component::Normal(value) = component else {
            return None;
        };
        let value = value.to_str()?;
        if value.contains('\\') {
            return None;
        }
        components.push(value);
    }

    if components.is_empty() {
        return None;
    }

    let path = components.join("/");
    if portable_stored_raw_components(&path).is_some() {
        Some(path)
    } else {
        None
    }
}

fn portable_stored_raw_components(raw: &str) -> Option<Vec<&str>> {
    if raw.is_empty()
        || raw.contains('\\')
        || raw.starts_with('/')
        || raw_starts_with_windows_drive_prefix(raw)
    {
        return None;
    }

    let components = raw.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .all(|component| !component.is_empty() && *component != "." && *component != "..")
    {
        Some(components)
    } else {
        None
    }
}

fn raw_starts_with_windows_drive_prefix(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn path_starts_with_windows_drive_prefix_using_forward_slash(path: &Path) -> bool {
    let Some(path) = path.as_os_str().to_str() else {
        return false;
    };

    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

impl CustomTheme {
    pub fn new(s: String, p: PathBuf) -> Self {
        CustomTheme { name: s, path: p }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

#[derive(Debug, Clone)]
pub struct WarpThemeConfig {
    theme_map: HashMap<ThemeKind, WarpTheme>,
}

impl WarpThemeConfig {
    pub fn new() -> Self {
        // preload with built-in themes
        let theme_map: HashMap<ThemeKind, WarpTheme> = HashMap::from_iter([
            (ThemeKind::Dark, dark_theme()),
            (ThemeKind::Light, light_theme()),
            (ThemeKind::SolarizedDark, solarized_dark()),
            (ThemeKind::SolarizedLight, solarized_light()),
            (ThemeKind::Dracula, dracula()),
            (ThemeKind::GruvboxDark, gruvbox_dark()),
            (ThemeKind::GruvboxLight, gruvbox_light()),
            (ThemeKind::JellyFish, jellyfish()),
            (ThemeKind::Koi, koi()),
            (ThemeKind::Leafy, leafy()),
            (ThemeKind::Marble, marble()),
            (ThemeKind::PinkCity, pink_city()),
            (ThemeKind::Snowy, snowy()),
            (ThemeKind::DarkCity, dark_city()),
            (ThemeKind::RedRock, red_rock()),
            (ThemeKind::CyberWave, cyber_wave()),
            (ThemeKind::WillowDream, willow_dream()),
            (ThemeKind::FancyDracula, fancy_dracula()),
            (ThemeKind::Phenomenon, phenomenon()),
            (ThemeKind::SolarFlare, solar_flare()),
            (ThemeKind::Adeberry, adeberry()),
        ]);
        WarpThemeConfig { theme_map }
    }

    pub fn add_new_theme(&mut self, theme_name: ThemeKind, theme: WarpTheme) {
        self.theme_map.insert(theme_name, theme);
    }

    pub fn file_to_theme(name: String, path: PathBuf) -> ThemeKind {
        CustomTheme::new(name, path).into()
    }

    pub fn theme_items(&self) -> impl Iterator<Item = (&ThemeKind, &WarpTheme)> {
        self.theme_map.iter()
    }

    pub fn theme(&self, name: &ThemeKind) -> WarpTheme {
        self.theme_map.get(name).cloned().unwrap_or_else(dark_theme)
    }
}

impl Default for WarpThemeConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RespectSystemTheme {
    #[default]
    Off,
    On(SelectedSystemThemes),
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    settings_value::SettingsValue,
)]
#[schemars(description = "Themes to use when following the system light/dark mode.")]
pub struct SelectedSystemThemes {
    #[schemars(description = "The theme to use in light mode.")]
    pub light: ThemeKind,
    #[schemars(description = "The theme to use in dark mode.")]
    pub dark: ThemeKind,
}

impl RespectSystemTheme {
    pub fn selected_system_themes(&self) -> Option<&SelectedSystemThemes> {
        match self {
            RespectSystemTheme::Off => None,
            RespectSystemTheme::On(selected) => Some(selected),
        }
    }
}

impl Default for SelectedSystemThemes {
    fn default() -> Self {
        Self {
            light: ThemeKind::Light,
            dark: ThemeKind::Dark,
        }
    }
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
