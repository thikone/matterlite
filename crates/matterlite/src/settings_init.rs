// SPDX-License-Identifier: AGPL-3.0-only
// Inspired by warp-terminal `app/src/settings/init.rs` @ 5bb7bcc6a; no code copied.

//! Sets up the preference stores and registers the settings groups.

use ml_themes::ThemeSettings;
use settings::{PrivatePreferences, PublicPreferences, SettingsManager};
use warp_core::paths;
use warpui::AppContext;
use warpui_extras::user_preferences::file_backed::FileBackedUserPreferences;
use warpui_extras::user_preferences::in_memory::InMemoryPreferences;
use warpui_extras::user_preferences::toml_backed::TomlBackedUserPreferences;

/// Public settings live in `settings.toml` (user-visible and editable); private ones in a JSON
/// file in the state directory. If `settings.toml` can't be parsed it is left untouched and the
/// defaults are used for this run.
pub fn init(ctx: &mut AppContext) {
    let settings_path = paths::settings_file_path();
    let (public, parse_error) = TomlBackedUserPreferences::new(settings_path.clone());
    if let Some(err) = parse_error {
        log::error!("{} could not be parsed: {err}", settings_path.display());
    }

    let private_path = paths::private_preferences_file_path();
    let private: Box<dyn warpui_extras::user_preferences::UserPreferences> =
        match FileBackedUserPreferences::new(private_path.clone()) {
            Ok(prefs) => Box::new(prefs),
            Err(err) => {
                log::error!("{} could not be opened: {err}", private_path.display());
                Box::<InMemoryPreferences>::default()
            }
        };

    ctx.add_singleton_model(move |_| PublicPreferences::new(Box::new(public)));
    ctx.add_singleton_model(move |_| PrivatePreferences::new(private));
    ctx.add_singleton_model(|_| SettingsManager::default());

    ThemeSettings::register(ctx);
}
