// SPDX-License-Identifier: AGPL-3.0-only

// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod root_view;

use warpui::{AddWindowOptions, platform};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("MatterLite {} starting", env!("CARGO_PKG_VERSION"));

    let app = platform::AppBuilder::new(
        platform::AppCallbacks::default(),
        Box::new(assets::Assets),
        None,
    );
    app.run(|ctx| {
        ctx.add_window(
            AddWindowOptions {
                title: Some("MatterLite".to_string()),
                ..Default::default()
            },
            root_view::RootView::new,
        );
    })?;
    Ok(())
}
