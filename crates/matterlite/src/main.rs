// SPDX-License-Identifier: AGPL-3.0-only

// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod config_watcher;
mod settings_init;

use ml_themes::AppearanceManager;
use warpui::{AddWindowOptions, SingletonEntity as _, platform};

fn main() -> anyhow::Result<()> {
    let log_dir = warp_core::paths::logs_dir();
    if let Err(err) = ml_logging::init(&log_dir) {
        // Without a log file, still log to stderr.
        eprintln!("Logging to {} failed: {err:#}", log_dir.display());
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    }
    log::info!("MatterLite {} starting", env!("CARGO_PKG_VERSION"));
    warp_core::features::init();
    match warp_core::paths::portable_root() {
        Some(root) => log::info!("Portable mode: data in {}", root.display()),
        None => log::info!("Config in {}", warp_core::paths::config_dir().display()),
    }

    let callbacks = platform::AppCallbacks {
        on_os_appearance_changed: Some(Box::new(|ctx| {
            AppearanceManager::handle(ctx).update(ctx, |manager, ctx| {
                manager.refresh_theme_state(ctx);
            });
        })),
        // Saves the WebSocket resume state and flushes the cache.
        on_will_terminate: Some(Box::new(ml_chat::shutdown)),
        // One window: closing it quits (on macOS too, until there is a way to reopen it).
        on_window_will_close: Some(Box::new(|_, ctx| {
            if ctx.window_ids().next().is_none() {
                ctx.terminate_app(platform::TerminationMode::Cancellable, None);
            }
        })),
        ..Default::default()
    };
    let app = platform::AppBuilder::new(callbacks, Box::new(assets::Assets), None);
    app.run(|ctx| {
        settings_init::init(ctx);
        if let Err(err) = ml_themes::appearance::register(&assets::Assets, ctx) {
            log::error!("Couldn't set up fonts and themes: {err:#}");
            ctx.terminate_app(warpui::platform::TerminationMode::ForceTerminate, None);
            return;
        }
        config_watcher::init(ctx);
        register_secure_storage(ctx);
        ml_editor::init(ctx);
        ml_themes::theme_chooser::init(ctx);
        ml_workspace::init(ctx);
        ml_chat::init(ctx);
        ctx.add_window(
            AddWindowOptions {
                title: Some("MatterLite".to_string()),
                ..Default::default()
            },
            ml_chat::RootView::new,
        );
        ml_chat::restore_session(ctx);
    })?;
    Ok(())
}

/// Session tokens (MatterLite.md §9): the Keychain on macOS, the Secret Service on Linux (none
/// running: log in on every start), DPAPI-encrypted files in `state/secure/` on Windows.
fn register_secure_storage(ctx: &mut warpui::AppContext) {
    #[cfg(windows)]
    {
        // The store writes its files without creating the directory.
        let dir = warp_core::paths::secure_state_dir();
        if let Err(err) = std::fs::create_dir_all(&dir) {
            log::error!("Can't create {}: {err}", dir.display());
        }
        warpui_extras::secure_storage::register_with_dir(SECURE_STORAGE_SERVICE, dir, ctx);
    }
    #[cfg(not(windows))]
    warpui_extras::secure_storage::register(SECURE_STORAGE_SERVICE, ctx);
}

const SECURE_STORAGE_SERVICE: &str = "MatterLite";
