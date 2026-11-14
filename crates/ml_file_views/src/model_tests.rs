// SPDX-License-Identifier: AGPL-3.0-only

use warp_core::ui::appearance::Appearance;
use warpui::App;

use super::*;
use crate::styles::{code_text_styles, document_text_styles};

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_| Appearance::mock());
}

fn add_model(app: &mut App, format: DocumentFormat, text: &str) -> ModelHandle<FileViewModel> {
    let styles = app.read(|ctx| {
        let appearance = Appearance::as_ref(ctx);
        match format {
            DocumentFormat::PlainText => code_text_styles(appearance),
            _ => document_text_styles(appearance),
        }
    });
    let model = app.add_model(|ctx| FileViewModel::new(format, styles, ctx));
    model.update(app, |model, ctx| model.load(text, ctx));
    model
}

#[test]
fn plain_text_round_trips() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let text = "fn main() {\n    println!(\"hi\");\n}";
        let model = add_model(&mut app, DocumentFormat::PlainText, text);
        model.read(&app, |model, ctx| {
            assert_eq!(model.text(ctx), text);
            assert_eq!(model.line_count(ctx), 3);
        });
    });
}

#[test]
fn markdown_is_parsed_into_blocks() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let model = add_model(
            &mut app,
            DocumentFormat::Markdown,
            "# Title\n\nSome **bold** text.\n\n- one\n- two\n",
        );
        model.read(&app, |model, ctx| {
            let markdown = model.text(ctx);
            assert!(markdown.contains("# Title"), "{markdown}");
            assert!(markdown.contains("**bold**"), "{markdown}");
            assert!(markdown.contains("one"), "{markdown}");
        });
    });
}

#[test]
fn notebook_cells_become_document_text() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let notebook = r##"{
            "cells": [
                {"cell_type": "markdown", "metadata": {}, "source": ["# Report"]},
                {"cell_type": "code", "execution_count": 1, "metadata": {}, "outputs": [],
                 "source": ["print(1 + 1)"]}
            ],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5
        }"##;
        let model = add_model(&mut app, DocumentFormat::Notebook, notebook);
        model.read(&app, |model, ctx| {
            let text = model.text(ctx);
            assert!(text.contains("Report"), "{text}");
            assert!(text.contains("print(1 + 1)"), "{text}");
            assert!(!text.contains("cell_type"), "{text}");
        });
    });
}

#[test]
fn languages_come_from_the_file_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let model = add_model(&mut app, DocumentFormat::PlainText, "");
        model.update(&mut app, |model, ctx| {
            assert!(model.set_language_for_path(Path::new("main.rs"), ctx));
            assert!(model.has_highlighting(ctx));
            assert!(!model.set_language_for_path(Path::new("notes.unknownext"), ctx));
        });
    });
}

#[test]
fn select_all_then_copy_puts_the_text_on_the_clipboard() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let text = "first line\nsecond line";
        let model = add_model(&mut app, DocumentFormat::PlainText, text);
        model.update(&mut app, |model, ctx| {
            model.select_all(ctx);
            model.copy(ctx);
        });
        let copied = app.update(|ctx| ctx.clipboard().read().plain_text);
        assert_eq!(copied, text);
    });
}
