// SPDX-License-Identifier: AGPL-3.0-only

use std::path::PathBuf;

use settings_value::SettingsValue as _;

use super::*;

#[test]
fn every_built_in_theme_kind_has_a_theme() {
    let config = WarpThemeConfig::new();
    let built_in = config.theme_items().count();
    // Every ThemeKind except the two custom variants is built in.
    assert_eq!(built_in, 21);
    assert!(
        config
            .theme_items()
            .all(|(kind, _)| !matches!(kind, ThemeKind::Custom(_) | ThemeKind::CustomBase16(_)))
    );
}

#[test]
fn unknown_theme_falls_back_to_dark() {
    let config = WarpThemeConfig::new();
    let missing = ThemeKind::Custom(CustomTheme::new("Missing".into(), PathBuf::from("x.yml")));
    assert_eq!(config.theme(&missing), config.theme(&ThemeKind::Dark));
}

#[test]
fn base16_custom_themes_are_detected_by_name() {
    let base16 = CustomTheme::new("Base16 Ocean".into(), PathBuf::from("ocean.yml"));
    assert!(matches!(
        ThemeKind::from(base16),
        ThemeKind::CustomBase16(_)
    ));
    let plain = CustomTheme::new("Ocean".into(), PathBuf::from("ocean.yml"));
    assert!(matches!(ThemeKind::from(plain), ThemeKind::Custom(_)));
}

#[test]
fn custom_theme_under_the_themes_dir_is_stored_relative() {
    let path = warp_core::paths::themes_dir().join("sub").join("mine.yml");
    let custom = CustomTheme::new("Mine".into(), path.clone());
    let stored = custom.to_file_value();
    assert_eq!(stored["path"], "sub/mine.yml");
    let restored = CustomTheme::from_file_value(&stored).expect("round trip");
    assert_eq!(restored.path(), path);
}

#[test]
fn custom_theme_outside_the_themes_dir_keeps_its_path() {
    let path = std::env::temp_dir().join("elsewhere.yml");
    let custom = CustomTheme::new("Elsewhere".into(), path.clone());
    let restored = CustomTheme::from_file_value(&custom.to_file_value()).expect("round trip");
    assert_eq!(restored.path(), path);
}

#[test]
fn theme_kind_matches_names_case_insensitively() {
    assert!(ThemeKind::SolarizedDark.matches("solar"));
    assert!(!ThemeKind::Dracula.matches("solar"));
}
