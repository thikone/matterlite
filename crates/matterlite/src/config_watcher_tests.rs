// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;

use ml_themes::{ThemeKind, ThemeSettings};
use settings::{PrivatePreferences, Setting as _};
use warpui::App;
use warpui_extras::user_preferences::in_memory::InMemoryPreferences;
use warpui_extras::user_preferences::toml_backed::TomlBackedUserPreferences;

use super::*;

fn event(modified: &[&Path]) -> BulkFilesystemWatcherEvent {
    BulkFilesystemWatcherEvent {
        modified: modified.iter().map(|path| path.to_path_buf()).collect(),
        ..Default::default()
    }
}

#[test]
fn events_are_matched_to_the_settings_file_and_themes_dir() {
    let dir = tempfile::tempdir().unwrap();
    let settings_file = dir.path().join("settings.toml");
    let themes_dir = dir.path().join("themes");
    fs::create_dir(&themes_dir).unwrap();
    let theme = themes_dir.join("nested").join("ocean.yaml");

    assert!(touches_path(&event(&[&settings_file]), &settings_file));
    assert!(!touches_path(&event(&[&theme]), &settings_file));
    assert!(touches_dir(&event(&[&theme]), &themes_dir));
    assert!(!touches_dir(&event(&[&settings_file]), &themes_dir));

    // Deletions and both ends of a rename count too.
    let deleted = BulkFilesystemWatcherEvent {
        deleted: [settings_file.clone()].into(),
        ..Default::default()
    };
    assert!(touches_path(&deleted, &settings_file));
    let renamed = BulkFilesystemWatcherEvent {
        moved: [(settings_file.clone(), dir.path().join("settings.toml.tmp"))].into(),
        ..Default::default()
    };
    assert!(touches_path(&renamed, &settings_file));
}

fn saved_theme(app: &App) -> ThemeKind {
    ThemeSettings::handle(app).read(app, |settings, _| settings.theme_kind.value().clone())
}

#[test]
fn reloading_applies_edits_and_keeps_settings_on_parse_errors() {
    // `settings.toml` storage is behind the SettingsFile flag, as in the app.
    warp_core::features::init();
    App::test((), |mut app| async move {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "[appearance.themes]\ntheme = \"koi\"\n").unwrap();

        let (public, error) = TomlBackedUserPreferences::new(path.clone());
        assert!(error.is_none());
        app.add_singleton_model(move |_| PublicPreferences::new(Box::new(public)));
        app.add_singleton_model(|_| PrivatePreferences::new(Box::<InMemoryPreferences>::default()));
        app.add_singleton_model(|_| SettingsManager::default());
        app.update(ThemeSettings::register);
        assert_eq!(saved_theme(&app), ThemeKind::Koi);

        fs::write(&path, "[appearance.themes]\ntheme = \"dracula\"\n").unwrap();
        app.update(reload_settings);
        assert_eq!(saved_theme(&app), ThemeKind::Dracula);

        // Half-written file: the current value stays.
        fs::write(&path, "[appearance.themes\ntheme = ").unwrap();
        app.update(reload_settings);
        assert_eq!(saved_theme(&app), ThemeKind::Dracula);

        // A value that doesn't fit is ignored: the current value stays.
        fs::write(&path, "[appearance.themes]\ntheme = \"no_such_theme\"\n").unwrap();
        app.update(reload_settings);
        assert_eq!(saved_theme(&app), ThemeKind::Dracula);

        // Removing the setting restores its default.
        fs::write(&path, "[appearance.themes]\ntheme = \"koi\"\n").unwrap();
        app.update(reload_settings);
        assert_eq!(saved_theme(&app), ThemeKind::Koi);
        fs::write(&path, "").unwrap();
        app.update(reload_settings);
        assert_eq!(saved_theme(&app), ThemeKind::default());
    });
}
