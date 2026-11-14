// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/appearance.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Keeps the app-wide [`Appearance`] in sync with the theme settings and the OS light/dark mode.

use anyhow::{Context as _, Result, anyhow};
pub use warp_core::ui::appearance::{Appearance, AppearanceEvent};
use warp_core::ui::theme::WarpTheme;
use warpui::elements::DEFAULT_UI_LINE_HEIGHT_RATIO;
use warpui::fonts::{FamilyId, Weight};
use warpui::{AppContext, AssetProvider, Entity, ModelContext, SingletonEntity};

use crate::custom_themes::load_theme_configs;
use crate::settings::{ThemeSettings, active_theme_kind};
use crate::theme::{ThemeKind, WarpThemeConfig};

/// Monospace font size used for code blocks and the code editor.
pub const DEFAULT_MONOSPACE_FONT_SIZE: f32 = 13.0;

/// Owns the theme registry and pushes theme changes into [`Appearance`]: when the theme
/// settings change, when the OS switches between light and dark mode, and while a theme is
/// previewed (transient theme) in the theme chooser.
///
/// The registry holds the built-in themes and the custom themes from the themes directory,
/// read at startup and again by [`Self::reload_themes`].
pub struct AppearanceManager {
    themes: WarpThemeConfig,
    transient_theme: Option<WarpTheme>,
}

impl AppearanceManager {
    pub(crate) fn new(themes: WarpThemeConfig, ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&ThemeSettings::handle(ctx), move |me, _, _event, ctx| {
            me.refresh_theme_state(ctx);
        });
        Self {
            themes,
            transient_theme: None,
        }
    }

    /// Re-reads the custom themes from the themes directory, then re-applies the active theme
    /// (an edited custom theme takes effect at once).
    pub fn reload_themes(&mut self, ctx: &mut ModelContext<Self>) {
        self.themes = load_theme_configs(&warp_core::paths::themes_dir());
        ctx.emit(AppearanceManagerEvent::ThemesChanged);
        self.refresh_theme_state(ctx);
    }

    /// The registry of available themes.
    pub fn themes(&self) -> &WarpThemeConfig {
        &self.themes
    }

    /// Re-derives the active theme (settings + OS mode, or the transient preview) and applies it.
    /// Call this when the OS appearance changes.
    pub fn refresh_theme_state(&mut self, ctx: &mut ModelContext<Self>) {
        let new_theme = match &self.transient_theme {
            Some(theme) => theme.clone(),
            None => {
                let theme_kind = active_theme_kind(ThemeSettings::as_ref(ctx), ctx);
                self.themes.theme(&theme_kind)
            }
        };
        Appearance::handle(ctx).update(ctx, |appearance, ctx| {
            appearance.set_theme(new_theme, ctx);
        })
    }

    /// Shows `theme` without changing the saved setting (theme chooser preview).
    pub fn set_transient_theme(&mut self, theme: &ThemeKind, ctx: &mut ModelContext<Self>) {
        self.transient_theme = Some(self.themes.theme(theme));
        self.refresh_theme_state(ctx);
    }

    /// Ends a preview started with [`Self::set_transient_theme`].
    pub fn clear_transient_theme(&mut self, ctx: &mut ModelContext<Self>) {
        self.transient_theme = None;
        self.refresh_theme_state(ctx);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AppearanceManagerEvent {
    /// The set of available themes changed.
    ThemesChanged,
}

impl Entity for AppearanceManager {
    type Event = AppearanceManagerEvent;
}

impl SingletonEntity for AppearanceManager {}

/// Loads the bundled fonts and registers the [`Appearance`] and [`AppearanceManager`]
/// singletons. [`ThemeSettings`] must be registered first.
pub fn register(assets: &dyn AssetProvider, ctx: &mut AppContext) -> Result<()> {
    let monospace = load_font_family(
        assets,
        "Hack",
        &[
            "bundled/fonts/hack/Hack-Italic.ttf",
            "bundled/fonts/hack/Hack-Bold.ttf",
            "bundled/fonts/hack/Hack-Regular.ttf",
            "bundled/fonts/hack/Hack-BoldItalic.ttf",
        ],
        ctx,
    )?;
    let ui = load_ui_font_family(assets, ctx)?;
    let password = load_font_family(
        assets,
        "PasswordCircle",
        &["bundled/fonts/password.ttf"],
        ctx,
    )?;

    let themes = load_theme_configs(&warp_core::paths::themes_dir());
    let theme = themes.theme(&active_theme_kind(ThemeSettings::as_ref(ctx), ctx));
    let appearance = Appearance::new(
        theme,
        monospace,
        DEFAULT_MONOSPACE_FONT_SIZE,
        Weight::Normal,
        ui,
        DEFAULT_UI_LINE_HEIGHT_RATIO,
        monospace,
        password,
    );
    ctx.add_singleton_model(move |_| appearance);
    ctx.add_singleton_model(move |ctx| AppearanceManager::new(themes, ctx));
    Ok(())
}

fn load_ui_font_family(assets: &dyn AssetProvider, ctx: &mut AppContext) -> Result<FamilyId> {
    // Segoe UI is what Windows users expect for UI text; elsewhere use the bundled Roboto.
    #[cfg(windows)]
    {
        let segoe = warpui::fonts::Cache::handle(ctx)
            .update(ctx, |cache, _| cache.load_system_font("Segoe UI"));
        if let Ok(family) = segoe {
            return Ok(family);
        }
    }
    load_font_family(
        assets,
        "Roboto",
        &[
            "bundled/fonts/roboto/Roboto-Italic.ttf",
            "bundled/fonts/roboto/Roboto-Bold.ttf",
            "bundled/fonts/roboto/Roboto-Regular.ttf",
            "bundled/fonts/roboto/Roboto-Medium.ttf",
            "bundled/fonts/roboto/RobotoFlex-Semibold.ttf",
            "bundled/fonts/roboto/Roboto-BoldItalic.ttf",
        ],
        ctx,
    )
}

fn load_font_family(
    assets: &dyn AssetProvider,
    name: &str,
    paths: &[&str],
    ctx: &mut AppContext,
) -> Result<FamilyId> {
    let bytes = paths
        .iter()
        .map(|path| {
            assets
                .get(path)
                .map(|data| data.into_owned())
                .with_context(|| format!("missing font asset {path}"))
        })
        .collect::<Result<Vec<_>>>()?;
    warpui::fonts::Cache::handle(ctx)
        .update(ctx, |cache, _| cache.load_family_from_bytes(name, bytes))
        .map_err(|err| anyhow!("couldn't load font family {name}: {err}"))
}
