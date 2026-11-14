// SPDX-License-Identifier: AGPL-3.0-only

//! The read-only spike of MatterLite.md §14.4: opens local files in [`FileView`]s.
//!
//! ```text
//! cargo run -p ml_file_views --example file_viewer -- README.md crates/ml_file_views/src/view.rs
//! ```
//!
//! Without arguments it opens the repository README and this crate's view source. Ctrl+1…9
//! switch files. Set `MATTERLITE_THEME` to a theme name (e.g. `dracula`) to try another theme.

use std::path::PathBuf;

use anyhow::anyhow;
use ml_file_views::FileView;
use ml_themes::{ThemeSettings, WarpThemeConfig};
use rust_embed::RustEmbed;
use settings::Setting as _;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, Hoverable,
    MainAxisSize, MouseStateHandle, ParentElement, Rect, Shrinkable, Stack, Text,
};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::{
    AddWindowOptions, AppContext, AssetProvider, Element, Entity, SingletonEntity as _,
    TypedActionView, View, ViewContext, ViewHandle, platform,
};
use warpui_extras::user_preferences::in_memory::InMemoryPreferences;

#[derive(RustEmbed)]
#[folder = "../../assets"]
struct Assets;

impl AssetProvider for Assets {
    fn get(&self, path: &str) -> anyhow::Result<std::borrow::Cow<'_, [u8]>> {
        <Assets as RustEmbed>::get(path)
            .map(|file| file.data)
            .ok_or_else(|| anyhow!("no asset exists at path {path}"))
    }
}

#[derive(Clone, Copy, Debug)]
enum ViewerAction {
    Show(usize),
}

struct Viewer {
    files: Vec<(String, ViewHandle<FileView>, MouseStateHandle)>,
    active: usize,
}

impl Viewer {
    fn new(paths: Vec<PathBuf>, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        let files: Vec<_> = paths
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let view = ctx.add_typed_action_view(|ctx| FileView::new(path, ctx));
                (name, view, MouseStateHandle::default())
            })
            .collect();
        if let Some((_, view, _)) = files.first() {
            ctx.focus(view);
        }
        Self { files, active: 0 }
    }

    fn render_tabs(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for (index, (name, _, mouse_state)) in self.files.iter().enumerate() {
            let active = index == self.active;
            let label = format!("{}  {name}", index + 1);
            let font = appearance.ui_font_family();
            let size = appearance.ui_font_size();
            let color = if active {
                theme.main_text_color(theme.background())
            } else {
                theme.sub_text_color(theme.background())
            };
            let background = if active {
                internal_colors::fg_overlay_2(theme)
            } else {
                internal_colors::fg_overlay_1(theme)
            };
            row.add_child(
                Hoverable::new(mouse_state.clone(), move |_| {
                    Container::new(
                        Text::new_inline(label, font, size)
                            .with_color(color.into_solid())
                            .finish(),
                    )
                    .with_horizontal_padding(12.)
                    .with_vertical_padding(7.)
                    .with_background(background)
                    .with_border(
                        Border::right(1.).with_border_fill(internal_colors::fg_overlay_3(theme)),
                    )
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(ViewerAction::Show(index)))
                .finish(),
            );
        }
        Container::new(ConstrainedBox::new(row.finish()).with_height(34.).finish())
            .with_border(Border::bottom(1.).with_border_fill(internal_colors::fg_overlay_3(theme)))
            .finish()
    }
}

impl Entity for Viewer {
    type Event = ();
}

impl View for Viewer {
    fn ui_name() -> &'static str {
        "Viewer"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(self.render_tabs(appearance));
        if let Some((_, view, _)) = self.files.get(self.active) {
            column.add_child(Shrinkable::new(1., ChildView::new(view).finish()).finish());
        }
        Stack::new()
            .with_child(
                Rect::new()
                    .with_background(appearance.theme().background())
                    .finish(),
            )
            .with_child(column.finish())
            .finish()
    }
}

impl TypedActionView for Viewer {
    type Action = ViewerAction;

    fn handle_action(&mut self, action: &ViewerAction, ctx: &mut ViewContext<Self>) {
        let ViewerAction::Show(index) = *action;
        if let Some((_, view, _)) = self.files.get(index) {
            self.active = index;
            ctx.focus(view);
            ctx.notify();
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    warp_core::features::init();

    let mut paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        paths = vec![root.join("../../README.md"), root.join("src/view.rs")];
    }

    let app = platform::AppBuilder::new(Default::default(), Box::new(Assets), None);
    app.run(move |ctx| {
        ctx.add_singleton_model(|_| {
            settings::PublicPreferences::new(Box::<InMemoryPreferences>::default())
        });
        ctx.add_singleton_model(|_| {
            settings::PrivatePreferences::new(Box::<InMemoryPreferences>::default())
        });
        ctx.add_singleton_model(|_| settings::SettingsManager::default());
        ThemeSettings::register(ctx);
        if let Ok(name) = std::env::var("MATTERLITE_THEME") {
            // "dracula", "solarized_dark", …: the theme's name in snake_case.
            let theme = WarpThemeConfig::new()
                .theme_items()
                .map(|(kind, _)| kind.clone())
                .find(|kind| kind.to_string().to_lowercase().replace(' ', "_") == name);
            match theme {
                Some(theme) => ThemeSettings::handle(ctx).update(ctx, |settings, ctx| {
                    let _ = settings.theme_kind.set_value(theme, ctx);
                }),
                None => eprintln!("Unknown theme {name}"),
            }
        }
        if let Err(err) = ml_themes::appearance::register(&Assets, ctx) {
            eprintln!("Couldn't load fonts and themes: {err:#}");
            return;
        }
        ml_file_views::init(ctx);

        use warpui::keymap::macros::*;
        ctx.register_fixed_bindings((1..=9).map(|number| {
            FixedBinding::new(
                format!("cmdorctrl-{number}"),
                ViewerAction::Show(number - 1),
                id!("Viewer"),
            )
        }));

        ctx.add_window(
            AddWindowOptions {
                title: Some("MatterLite file viewer spike".to_string()),
                ..Default::default()
            },
            move |ctx| Viewer::new(paths, ctx),
        );
    })?;
    Ok(())
}
