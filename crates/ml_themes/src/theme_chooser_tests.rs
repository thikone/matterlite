// SPDX-License-Identifier: AGPL-3.0-only

use warpui::platform::WindowStyle;
use warpui::{App, ModelHandle};
use warpui_extras::user_preferences::in_memory::InMemoryPreferences;

use super::*;
use crate::theme::WarpThemeConfig;

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_| {
        settings::PublicPreferences::new(Box::<InMemoryPreferences>::default())
    });
    app.add_singleton_model(|_| {
        settings::PrivatePreferences::new(Box::<InMemoryPreferences>::default())
    });
    app.add_singleton_model(|_| settings::SettingsManager::default());
    app.update(ThemeSettings::register);
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(|ctx| AppearanceManager::new(WarpThemeConfig::new(), ctx));
    app.update(ml_editor::init);
    app.update(init);
}

fn add_chooser(app: &mut App) -> ViewHandle<ThemeChooser> {
    let chooser = app
        .add_window(WindowStyle::NotStealFocus, ThemeChooser::new)
        .1;
    chooser.update(app, |chooser, ctx| chooser.open(ctx));
    chooser
}

fn saved_theme(app: &App) -> ThemeKind {
    ThemeSettings::handle(app).read(app, |settings, _| settings.theme_kind.value().clone())
}

fn search(app: &mut App, chooser: &ViewHandle<ThemeChooser>, query: &str) {
    chooser.update(app, |chooser, ctx| {
        chooser
            .search_editor
            .update(ctx, |editor, ctx| editor.user_insert(query, ctx));
    });
}

#[derive(Default)]
struct Closed(usize);

impl Entity for Closed {
    type Event = ();
}

fn count_closes(app: &mut App, chooser: &ViewHandle<ThemeChooser>) -> ModelHandle<Closed> {
    app.add_model(|ctx| {
        ctx.subscribe_to_view(chooser, |closed: &mut Closed, _, event, _| match event {
            ThemeChooserEvent::Close => closed.0 += 1,
        });
        Closed::default()
    })
}

#[test]
fn lists_the_built_in_themes_sorted_and_selects_the_saved_one() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let chooser = add_chooser(&mut app);
        chooser.read(&app, |chooser, _| {
            let themes: Vec<_> = chooser.themes().cloned().collect();
            // Warp's 21, Mattermost's 5 and the account's theme, which comes first.
            assert_eq!(themes.len(), 27);
            assert!(themes.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(themes[0], ThemeKind::MattermostAccount);
            assert_eq!(chooser.mode(), ThemeChooserMode::SystemAgnostic);
            assert_eq!(chooser.selected(), Some(&ThemeKind::MattermostAccount));
        });
    });
}

#[test]
fn moving_down_applies_and_saves_the_next_theme() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let chooser = add_chooser(&mut app);
        let next = chooser.read(&app, |chooser, _| {
            let themes: Vec<_> = chooser.themes().cloned().collect();
            let saved = themes
                .iter()
                .position(|kind| *kind == ThemeKind::default())
                .unwrap();
            themes[saved + 1].clone()
        });

        chooser.update(&mut app, |chooser, ctx| {
            chooser.handle_action(&ThemeChooserAction::Down, ctx)
        });

        assert_eq!(saved_theme(&app), next);
        chooser.read(&app, |chooser, _| {
            assert_eq!(chooser.selected(), Some(&next))
        });
    });
}

#[test]
fn search_narrows_the_list_that_up_and_down_move_through() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let chooser = add_chooser(&mut app);
        search(&mut app, &chooser, "solar");

        chooser.update(&mut app, |chooser, ctx| {
            // "Dark" isn't visible, so Down starts at the first match.
            chooser.handle_action(&ThemeChooserAction::Down, ctx);
            chooser.handle_action(&ThemeChooserAction::Down, ctx);
            chooser.handle_action(&ThemeChooserAction::Down, ctx);
        });
        let saved = saved_theme(&app);
        assert!(saved.matches("solar"), "{saved}");

        // Moving down past the last match stays on it.
        chooser.update(&mut app, |chooser, ctx| {
            chooser.handle_action(&ThemeChooserAction::Down, ctx)
        });
        assert_eq!(saved_theme(&app), saved);
    });
}

#[test]
fn enter_closes_only_with_a_visible_selection() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let chooser = add_chooser(&mut app);
        let closed = count_closes(&mut app, &chooser);

        search(&mut app, &chooser, "no such theme");
        chooser.update(&mut app, |chooser, ctx| {
            chooser.handle_action(&ThemeChooserAction::Enter, ctx)
        });
        closed.read(&app, |closed, _| assert_eq!(closed.0, 0));

        chooser.update(&mut app, |chooser, ctx| {
            chooser.handle_action(&ThemeChooserAction::Close, ctx)
        });
        closed.read(&app, |closed, _| assert_eq!(closed.0, 1));
        chooser.read(&app, |chooser, ctx| {
            assert_eq!(chooser.selected(), None);
            assert!(chooser.search_editor.as_ref(ctx).is_empty(ctx));
        });
    });
}
