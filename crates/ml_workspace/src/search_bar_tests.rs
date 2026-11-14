// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn empty_query_does_not_filter() {
    assert!(Matcher::new("", false, false).is_none());
    assert!(Matcher::new("", true, true).is_none());
}

#[test]
fn literal_is_case_insensitive_by_default() {
    let matcher = Matcher::new("Deploy", false, false).unwrap();
    assert!(matcher.is_match("the deploy is done"));
    assert!(matcher.is_match("DEPLOY"));
    assert_eq!(matcher.count("deploy, Deploy, DEPLOY"), 3);
}

#[test]
fn case_sensitive_literal() {
    let matcher = Matcher::new("Deploy", false, true).unwrap();
    assert!(!matcher.is_match("the deploy is done"));
    assert_eq!(matcher.count("deploy, Deploy, DEPLOY"), 1);
}

#[test]
fn regex_respects_the_case_option() {
    let insensitive = Matcher::new(r"err(or)?\b", true, false).unwrap();
    assert!(insensitive.is_match("ERROR: disk full"));
    assert_eq!(insensitive.count("err, error, errors"), 2);

    let sensitive = Matcher::new("^Build", true, true).unwrap();
    assert!(sensitive.is_match("Build passed"));
    assert!(!sensitive.is_match("build passed"));
}

#[test]
fn half_typed_regex_does_not_filter() {
    assert!(Matcher::new("deploy(", true, false).is_none());
    assert!(Matcher::new("[a-", true, false).is_none());
    // The same text is a fine literal.
    assert!(Matcher::new("deploy(", false, false).is_some());
}

#[test]
fn regex_characters_are_literal_without_the_regex_option() {
    let matcher = Matcher::new("a.b", false, false).unwrap();
    assert!(matcher.is_match("a.b"));
    assert!(!matcher.is_match("axb"));
}

mod view {
    use warpui::platform::WindowStyle;
    use warpui::{App, ModelHandle};
    use warpui_extras::user_preferences::in_memory::InMemoryPreferences;

    use super::*;

    fn initialize_app(app: &mut App) {
        app.add_singleton_model(|_| {
            settings::PublicPreferences::new(Box::<InMemoryPreferences>::default())
        });
        app.add_singleton_model(|_| {
            settings::PrivatePreferences::new(Box::<InMemoryPreferences>::default())
        });
        app.add_singleton_model(|_| settings::SettingsManager::default());
        app.add_singleton_model(|_| Appearance::mock());
        app.update(ml_editor::init);
    }

    fn scope_toggles() -> SearchBarOptions {
        let toggle = |id, on| ToggleSpec {
            id,
            icon: Icon::Search,
            tooltip: id,
            on,
        };
        SearchBarOptions {
            placeholder: "Search".into(),
            query_options: true,
            toggle_groups: vec![
                ToggleGroup {
                    toggles: vec![toggle("messages", true), toggle("files", false)],
                    keep_one_on: true,
                },
                ToggleGroup {
                    toggles: vec![toggle("filter", false)],
                    keep_one_on: false,
                },
            ],
        }
    }

    fn add_bar(app: &mut App) -> ViewHandle<SearchBarView> {
        app.add_window(WindowStyle::NotStealFocus, |ctx| {
            SearchBarView::new(scope_toggles(), ctx)
        })
        .1
    }

    /// Records the bar's events by name.
    #[derive(Default)]
    struct Events(Vec<String>);

    impl Entity for Events {
        type Event = ();
    }

    fn record_events(app: &mut App, bar: &ViewHandle<SearchBarView>) -> ModelHandle<Events> {
        app.add_model(|ctx| {
            ctx.subscribe_to_view(bar, |events: &mut Events, _, event, _| {
                events.0.push(match event {
                    SearchBarEvent::QueryChanged => "query".to_string(),
                    SearchBarEvent::ToggleChanged { id, on } => format!("{id}={on}"),
                    SearchBarEvent::Submit => "submit".to_string(),
                    SearchBarEvent::Cleared => "cleared".to_string(),
                    SearchBarEvent::Dismissed => "dismissed".to_string(),
                    SearchBarEvent::Navigate(key) => format!("{key:?}"),
                });
            });
            Events::default()
        })
    }

    /// The events recorded so far, sorted, and forgets them. Sorted because the editor
    /// delivers `Edited` after the update that caused it, so within one update the bar's own
    /// events can arrive first.
    fn take_sorted(app: &mut App, events: &ModelHandle<Events>) -> Vec<String> {
        events.update(app, |events, _| {
            let mut taken = std::mem::take(&mut events.0);
            taken.sort();
            taken
        })
    }

    #[test]
    fn typing_builds_the_matcher_and_enter_submits() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let bar = add_bar(&mut app);
            let events = record_events(&mut app, &bar);

            bar.update(&mut app, |bar, ctx| {
                bar.editor.update(ctx, |editor, ctx| {
                    editor.user_insert("Deploy", ctx);
                    editor.handle_action(&ml_editor::EditorAction::Enter, ctx);
                });
            });

            bar.read(&app, |bar, ctx| {
                assert_eq!(bar.query(ctx), "Deploy");
                assert!(bar.matcher(ctx).unwrap().is_match("deploy done"));
            });
            assert_eq!(take_sorted(&mut app, &events), ["query", "submit"]);
        });
    }

    #[test]
    fn escape_clears_then_dismisses() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let bar = add_bar(&mut app);
            bar.update(&mut app, |bar, ctx| {
                bar.editor
                    .update(ctx, |editor, ctx| editor.user_insert("abc", ctx));
            });
            let events = record_events(&mut app, &bar);

            bar.update(&mut app, |bar, ctx| {
                bar.editor.update(ctx, |editor, ctx| editor.escape(ctx));
            });
            bar.read(&app, |bar, ctx| assert_eq!(bar.query(ctx), ""));
            assert_eq!(take_sorted(&mut app, &events), ["cleared", "query"]);

            bar.update(&mut app, |bar, ctx| {
                bar.editor.update(ctx, |editor, ctx| editor.escape(ctx));
            });
            assert_eq!(take_sorted(&mut app, &events), ["dismissed"]);
        });
    }

    #[test]
    fn the_last_scope_toggle_stays_on() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let bar = add_bar(&mut app);
            let events = record_events(&mut app, &bar);

            bar.update(&mut app, |bar, ctx| {
                // "messages" is the only scope on: refused.
                bar.handle_action(&SearchBarAction::Toggle { group: 0, index: 0 }, ctx);
                // Turn "files" on, then "messages" off.
                bar.handle_action(&SearchBarAction::Toggle { group: 0, index: 1 }, ctx);
                bar.handle_action(&SearchBarAction::Toggle { group: 0, index: 0 }, ctx);
                // "filter" has no such rule.
                bar.handle_action(&SearchBarAction::Toggle { group: 1, index: 0 }, ctx);
                bar.handle_action(&SearchBarAction::Toggle { group: 1, index: 0 }, ctx);
            });

            bar.read(&app, |bar, _| {
                assert!(!bar.is_toggle_on("messages"));
                assert!(bar.is_toggle_on("files"));
                assert!(!bar.is_toggle_on("filter"));
                assert!(!bar.is_toggle_on("unknown"));
            });
            events.read(&app, |events, _| {
                assert_eq!(
                    events.0,
                    [
                        "files=true",
                        "messages=false",
                        "filter=true",
                        "filter=false"
                    ]
                );
            });
        });
    }

    #[test]
    fn query_options_change_the_matcher() {
        App::test((), |mut app| async move {
            initialize_app(&mut app);
            let bar = add_bar(&mut app);
            bar.update(&mut app, |bar, ctx| {
                bar.set_query("^Build", ctx);
                bar.handle_action(&SearchBarAction::ToggleRegex, ctx);
                bar.handle_action(&SearchBarAction::ToggleCaseSensitive, ctx);
            });
            bar.read(&app, |bar, ctx| {
                let matcher = bar.matcher(ctx).unwrap();
                assert!(matcher.is_match("Build passed"));
                assert!(!matcher.is_match("build passed"));
            });
        });
    }
}
