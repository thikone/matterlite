// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/user_config/util.rs` and `app/src/user_config/native.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Custom themes: one Warp-format YAML theme per `.yaml`/`.yml` file under the themes
//! directory (`warp_core::paths::themes_dir()`, MatterLite.md §11.1). Subdirectories are
//! searched too. A file without a `name` is named after the file (`solarized_dark.yaml` →
//! "Solarized Dark"). Files that don't parse are logged and skipped.

use std::fs;
use std::path::Path;

use anyhow::Result;
use walkdir::{DirEntry, WalkDir};

use crate::theme::{ThemeKind, WarpTheme, WarpThemeConfig};

const THEME_FILE_SUFFIXES: &[&str] = &[".yaml", ".yml"];

/// The built-in themes plus the custom themes found under `theme_dir`.
pub fn load_theme_configs(theme_dir: &Path) -> WarpThemeConfig {
    let mut config = WarpThemeConfig::new();
    for (kind, theme) in load_custom_themes(theme_dir) {
        config.add_new_theme(kind, theme);
    }
    config
}

/// The custom themes found under `theme_dir`; empty if it doesn't exist.
pub fn load_custom_themes(theme_dir: &Path) -> Vec<(ThemeKind, WarpTheme)> {
    if !theme_dir.is_dir() {
        return Vec::new();
    }
    WalkDir::new(theme_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| parse_theme_file(&entry))
        .collect()
}

fn parse_theme_file(entry: &DirEntry) -> Option<(ThemeKind, WarpTheme)> {
    // `metadata` follows links; an error means the file was deleted meanwhile.
    if !entry.metadata().ok()?.is_file() {
        return None;
    }
    let file_name = entry.file_name().to_str()?;
    if !is_theme_file(file_name) {
        return None;
    }
    let mut theme = match read_theme(entry.path()) {
        Ok(theme) => theme,
        Err(err) => {
            log::warn!("Skipping theme file {}: {err:#}", entry.path().display());
            return None;
        }
    };
    let name = theme.name().unwrap_or_else(|| {
        let name = file_name_to_theme_name(file_name);
        theme.set_name(name.clone());
        name
    });
    Some((
        WarpThemeConfig::file_to_theme(name, entry.path().to_path_buf()),
        theme,
    ))
}

fn read_theme(path: &Path) -> Result<WarpTheme> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&contents)?)
}

fn is_theme_file(file_name: &str) -> bool {
    THEME_FILE_SUFFIXES
        .iter()
        .any(|suffix| file_name.len() > suffix.len() && file_name.ends_with(suffix))
}

/// `solarized_dark.yaml` and `SOLARIZED_DARK.yml` → "Solarized Dark"; `_` separates words.
pub fn file_name_to_theme_name(file_name: &str) -> String {
    let stem = THEME_FILE_SUFFIXES
        .iter()
        .find_map(|suffix| file_name.strip_suffix(suffix))
        .unwrap_or(file_name);
    stem.split('_')
        .map(title_case)
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case(word: &str) -> String {
    let lowercase = word.to_lowercase();
    let mut chars = lowercase.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "custom_themes_tests.rs"]
mod tests;
