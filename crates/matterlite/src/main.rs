// SPDX-License-Identifier: AGPL-3.0-only

// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod settings_init;

use ml_themes::AppearanceManager;
use warpui::{AddWindowOptions, SingletonEntity as _, platform};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
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
        ml_workspace::init(ctx);
        ctx.add_window(
            AddWindowOptions {
                title: Some("MatterLite".to_string()),
                ..Default::default()
            },
            ml_workspace::Workspace::new,
        );
    })?;
    Ok(())
}
