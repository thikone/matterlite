// SPDX-License-Identifier: AGPL-3.0-only

use warpui::platform::WindowStyle;
use warpui::{App, ViewHandle};
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
    app.update(init);
}

fn add_editor(
    app: &mut App,
    build: impl FnOnce(&mut ViewContext<EditorView>) -> EditorView,
) -> ViewHandle<EditorView> {
    app.add_window(WindowStyle::NotStealFocus, build).1
}

/// Records the editor's enter events.
#[derive(Default)]
struct EnterEvents(Vec<&'static str>);

impl Entity for EnterEvents {
    type Event = ();
}

fn record_enter_events(app: &mut App, editor: &ViewHandle<EditorView>) -> ModelHandle<EnterEvents> {
    app.add_model(|ctx| {
        ctx.subscribe_to_view(editor, |events: &mut EnterEvents, _, event, _| {
            let name = match event {
                Event::Enter => "enter",
                Event::ShiftEnter => "shift-enter",
                _ => return,
            };
            events.0.push(name);
        });
        EnterEvents::default()
    })
}

#[test]
fn test_single_line_editor_inserts_text() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| {
            EditorView::single_line(Default::default(), ctx)
        });

        editor.update(&mut app, |editor, ctx| {
            assert!(editor.is_single_line());
            assert!(editor.is_empty(ctx));
            editor.user_insert("hello", ctx);
            editor.user_insert(" world", ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "hello world");
            assert!(editor.single_cursor_at_buffer_end(ctx));
        });
    });
}

#[test]
fn test_single_line_editor_emits_enter_instead_of_inserting_newlines() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| {
            EditorView::single_line(Default::default(), ctx)
        });
        let events = record_enter_events(&mut app, &editor);

        editor.update(&mut app, |editor, ctx| {
            editor.user_insert("query", ctx);
            editor.enter(ctx);
            editor.shift_enter(ctx);
            editor.newline(ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "query");
        });
        events.read(&app, |events, _| {
            assert_eq!(events.0, ["enter", "shift-enter", "shift-enter"]);
        });
    });
}

#[test]
fn test_multi_line_editor_inserts_newlines() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| EditorView::new(Default::default(), ctx));
        let events = record_enter_events(&mut app, &editor);

        editor.update(&mut app, |editor, ctx| {
            editor.user_insert("first", ctx);
            editor.shift_enter(ctx);
            editor.user_insert("second", ctx);
            editor.enter(ctx);
        });

        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "first\nsecond");
            assert!(editor.single_cursor_on_last_line(ctx));
            assert!(!editor.single_cursor_on_first_line(ctx));
        });
        events.read(&app, |events, _| assert_eq!(events.0, ["enter"]));
    });
}

#[test]
fn test_undo_and_redo() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| {
            EditorView::single_line(Default::default(), ctx)
        });

        editor.update(&mut app, |editor, ctx| {
            editor.user_insert("abc", ctx);
            editor.select_all(ctx);
            editor.backspace(ctx);
        });
        editor.read(&app, |editor, ctx| assert!(editor.is_empty(ctx)));

        editor.update(&mut app, |editor, ctx| editor.undo(ctx));
        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "abc")
        });

        editor.update(&mut app, |editor, ctx| editor.redo(ctx));
        editor.read(&app, |editor, ctx| assert!(editor.is_empty(ctx)));
    });
}

#[test]
fn test_word_deletion_and_cursor_movement() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| {
            EditorView::single_line(Default::default(), ctx)
        });

        editor.update(&mut app, |editor, ctx| {
            editor.user_insert("one two three", ctx);
            editor.delete_word_left(ctx);
        });
        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "one two ")
        });

        editor.update(&mut app, |editor, ctx| {
            editor.move_to_line_start(ctx);
            editor.user_insert(">", ctx);
        });
        editor.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), ">one two ")
        });
    });
}

#[test]
fn test_input_restrictions() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let limited = add_editor(&mut app, |ctx| {
            EditorView::single_line(
                SingleLineEditorOptions {
                    max_buffer_len: Some(5),
                    ..Default::default()
                },
                ctx,
            )
        });
        let numeric = add_editor(&mut app, |ctx| {
            EditorView::single_line(
                SingleLineEditorOptions {
                    valid_input_type: ValidInputType::PositiveInteger,
                    ..Default::default()
                },
                ctx,
            )
        });

        limited.update(&mut app, |editor, ctx| {
            editor.user_insert("12345", ctx);
            editor.user_insert("6", ctx);
        });
        limited.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "12345")
        });

        numeric.update(&mut app, |editor, ctx| {
            editor.user_insert("42", ctx);
            editor.user_insert("x", ctx);
        });
        numeric.read(&app, |editor, ctx| {
            assert_eq!(editor.buffer_text(ctx), "42")
        });
    });
}

#[test]
fn test_placeholder_and_buffer_text() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let editor = add_editor(&mut app, |ctx| {
            EditorView::single_line(Default::default(), ctx)
        });

        editor.update(&mut app, |editor, ctx| {
            editor.set_placeholder_text("Search", ctx);
            assert!(editor.placeholder_text_exists());

            editor.set_buffer_text("preset", ctx);
            assert_eq!(editor.buffer_text(ctx), "preset");

            editor.clear_buffer(ctx);
            assert!(editor.is_empty(ctx));
        });
    });
}
