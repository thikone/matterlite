// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/warp_managed_paths_watcher.rs`, `app/src/user_config/native.rs`
// and `handle_warp_config_change` in `app/src/settings/init.rs` @ 5bb7bcc6a (watch the config
// directory, reload settings.toml and the custom themes when they change); the code is new.

//! Hot reload of the config directory: edits to `settings.toml` apply while MatterLite runs, and
//! custom themes are re-read when files in the themes directory change.
//!
//! A `settings.toml` that doesn't parse (for example while it is being written) leaves the
//! current settings in place. A value that doesn't fit its setting is logged and that setting
//! keeps its current value; a setting removed from the file goes back to its default.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ml_themes::AppearanceManager;
use notify_debouncer_full::notify::{RecursiveMode, WatchFilter};
use settings::{PublicPreferences, SettingsManager};
use warp_core::paths;
use warpui::{AppContext, Entity, ModelContext, ModelHandle, SingletonEntity};
use watcher::{BulkFilesystemWatcher, BulkFilesystemWatcherEvent};

/// Editors write a file in several steps; changes within this window are handled once.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Watches the config directory. Kept alive as a singleton.
pub struct ConfigWatcher {
    _watcher: ModelHandle<BulkFilesystemWatcher>,
    settings_file: PathBuf,
    themes_dir: PathBuf,
}

impl ConfigWatcher {
    fn new(config_dir: PathBuf, ctx: &mut ModelContext<Self>) -> Self {
        let watcher = ctx.add_model(|ctx| BulkFilesystemWatcher::new(DEBOUNCE, ctx));
        ctx.subscribe_to_model(&watcher, |me, _, event, ctx| me.handle_event(event, ctx));
        let registered = watcher.update(ctx, |watcher, _| {
            watcher.register_path(
                &config_dir,
                WatchFilter::accept_all(),
                RecursiveMode::Recursive,
            )
        });
        let watched = config_dir.display().to_string();
        ctx.background_executor()
            .spawn(async move {
                match registered.await {
                    Ok(()) => log::info!("Watching {watched} for settings and theme changes"),
                    Err(err) => log::warn!("Couldn't watch {watched}: {err:#}"),
                }
            })
            .detach();
        Self {
            _watcher: watcher,
            settings_file: paths::settings_file_path(),
            themes_dir: paths::themes_dir(),
        }
    }

    fn handle_event(&mut self, event: &BulkFilesystemWatcherEvent, ctx: &mut ModelContext<Self>) {
        if touches_path(event, &self.settings_file) {
            reload_settings(ctx);
        }
        if touches_dir(event, &self.themes_dir) {
            log::info!("Custom themes changed; reloading them");
            AppearanceManager::handle(ctx).update(ctx, |manager, ctx| manager.reload_themes(ctx));
        }
    }
}

impl Entity for ConfigWatcher {
    type Event = ();
}

impl SingletonEntity for ConfigWatcher {}

/// Starts watching the config directory, creating it if needed. Call after the settings and
/// [`AppearanceManager`] are registered.
pub fn init(ctx: &mut AppContext) {
    let config_dir = paths::config_dir();
    if let Err(err) = std::fs::create_dir_all(&config_dir) {
        log::warn!(
            "Couldn't create {}; settings won't reload while running: {err}",
            config_dir.display()
        );
        return;
    }
    ctx.add_singleton_model(move |ctx| ConfigWatcher::new(config_dir, ctx));
}

/// Re-reads `settings.toml` and pushes the values into every public setting.
pub fn reload_settings(ctx: &mut AppContext) {
    let preferences = PublicPreferences::as_ref(ctx);
    if let Err(err) = preferences.reload_from_disk() {
        log::error!(
            "{} could not be parsed; keeping the current settings: {err}",
            paths::settings_file_path().display()
        );
        return;
    }
    let failed = SettingsManager::handle(ctx)
        .update(ctx, |manager, ctx| manager.reload_all_public_settings(ctx));
    if failed.is_empty() {
        log::info!("Reloaded {}", paths::settings_file_path().display());
    } else {
        log::error!(
            "{} has invalid values; keeping the current values of: {}",
            paths::settings_file_path().display(),
            failed.join(", ")
        );
    }
}

/// The paths the event reports, both sides of a rename included.
fn event_paths(event: &BulkFilesystemWatcherEvent) -> impl Iterator<Item = &PathBuf> {
    event
        .added
        .iter()
        .chain(&event.modified)
        .chain(&event.deleted)
        .chain(event.moved.keys())
        .chain(event.moved.values())
}

/// Event paths can be spelled differently from ours (case on Windows, symlinks), so both sides
/// are compared as given and canonicalised.
fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((dunce::canonicalize(a), dunce::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

fn touches_path(event: &BulkFilesystemWatcherEvent, path: &Path) -> bool {
    event_paths(event).any(|changed| same_path(changed, path))
}

fn touches_dir(event: &BulkFilesystemWatcherEvent, dir: &Path) -> bool {
    let canonical_dir = dunce::canonicalize(dir).ok();
    event_paths(event).any(|changed| {
        changed.starts_with(dir)
            || canonical_dir
                .as_ref()
                .is_some_and(|canonical| changed.starts_with(canonical))
    })
}

#[cfg(test)]
#[path = "config_watcher_tests.rs"]
mod tests;
