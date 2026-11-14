// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;

use warpui::platform::WindowStyle;
use warpui::{App, ViewHandle};

use super::*;

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_| Appearance::mock());
    app.update(init);
}

fn open(app: &mut App, path: &Path) -> ViewHandle<FileView> {
    let path = path.to_path_buf();
    app.add_window(WindowStyle::NotStealFocus, move |ctx| {
        FileView::new(path, ctx)
    })
    .1
}

#[test]
fn markdown_toggles_between_rendered_and_raw() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        fs::write(&path, "# Notes\n\n**bold**\n").unwrap();
        let view = open(&mut app, &path);

        view.read(&app, |view, ctx| {
            assert_eq!(view.kind(), FileViewKind::Markdown);
            assert!(!view.is_showing_raw());
            let model = view.active_model().unwrap().as_ref(ctx);
            assert_eq!(model.format(), DocumentFormat::Markdown);
        });

        view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewAction::ToggleRaw, ctx)
        });
        view.read(&app, |view, ctx| {
            assert!(view.is_showing_raw());
            let model = view.active_model().unwrap().as_ref(ctx);
            assert_eq!(model.format(), DocumentFormat::PlainText);
            // The source, markup included.
            assert_eq!(model.text(ctx), "# Notes\n\n**bold**\n");
        });

        view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewAction::ToggleRaw, ctx)
        });
        view.read(&app, |view, _| assert!(!view.is_showing_raw()));
    });
}

#[test]
fn code_files_are_plain_text_and_ignore_the_raw_toggle() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        let view = open(&mut app, &path);

        view.update(&mut app, |view, ctx| {
            view.handle_action(&FileViewAction::ToggleRaw, ctx);
            view.handle_action(&FileViewAction::Page { down: true }, ctx);
            view.handle_action(&FileViewAction::Scroll(Pixels::new(-40.)), ctx);
        });
        view.read(&app, |view, ctx| {
            assert_eq!(view.kind(), FileViewKind::Code);
            assert!(!view.is_showing_raw());
            let model = view.active_model().unwrap().as_ref(ctx);
            assert_eq!(model.format(), DocumentFormat::PlainText);
            assert!(model.has_highlighting(ctx));
        });
    });
}

#[test]
fn files_that_are_not_text_get_a_placeholder() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("dump.dat");
        fs::write(&binary, [0u8, 1, 2, 255]).unwrap();
        let image = dir.path().join("photo.png");
        fs::write(&image, [137u8, 80, 78, 71]).unwrap();

        for (path, kind) in [
            (binary, FileViewKind::External),
            (image, FileViewKind::Image),
            (dir.path().join("missing.dat"), FileViewKind::External),
        ] {
            let view = open(&mut app, &path);
            view.read(&app, |view, _| {
                assert_eq!(view.kind(), kind);
                assert!(view.active_model().is_none());
            });
        }
    });
}
