// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/code/editor/model.rs` and `app/src/notebooks/editor/model.rs`
// @ 5bb7bcc6a (buffer, selection and render-state wiring; syntax highlighting through a syntax
// tree); the code is new and read-only.

//! The document behind a [`crate::FileView`]: a `warp_editor` buffer with its selection and
//! render state, loaded from a file's text as Markdown, a Jupyter notebook or plain text. Code
//! is highlighted with `syntax_tree` in the theme's ANSI colours.

use std::path::Path;
use std::sync::Arc;

use languages::{Language, language_by_local_filename};
use rangemap::RangeSet;
use string_offset::CharOffset;
use syntax_tree::{ColorMap, DecorationStateEvent, SyntaxTreeState};
use warp_core::ui::appearance::Appearance;
use warp_editor::content::buffer::{
    Buffer, BufferEditAction, BufferEvent, EditOrigin, InitialBufferState, ShouldAutoscroll,
};
use warp_editor::content::selection_model::BufferSelectionModel;
use warp_editor::content::text::{IndentBehavior, TextStyles};
use warp_editor::content::version::BufferVersion;
use warp_editor::decoration::DecorationLayer as _;
use warp_editor::editor::TextDecoration;
use warp_editor::model::{CoreEditorModel, RichTextEditorModel};
use warp_editor::render::model::{RenderEvent, RenderState, RichTextStyles, WidthSetting};
use warp_editor::selection::{SelectionMode, SelectionModel};
use warpui::text::word_boundaries::WordBoundariesPolicy;
use warpui::{AppContext, Entity, ModelAsRef, ModelContext, ModelHandle, SingletonEntity as _};

/// How the file's text is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentFormat {
    Markdown,
    Notebook,
    /// One monospace line per paragraph, highlighted when the language is known.
    PlainText,
}

#[derive(Clone, Copy, Debug)]
pub enum FileViewModelEvent {
    /// New syntax highlighting is ready; repaint.
    HighlightingUpdated,
}

pub struct FileViewModel {
    content: ModelHandle<Buffer>,
    buffer_selection: ModelHandle<BufferSelectionModel>,
    selection: ModelHandle<SelectionModel>,
    render_state: ModelHandle<RenderState>,
    syntax_tree: ModelHandle<SyntaxTreeState>,
    format: DocumentFormat,
}

impl FileViewModel {
    pub fn new(
        format: DocumentFormat,
        styles: RichTextStyles,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        // Read-only: indentation never applies.
        let content = ctx.add_model(|_| Buffer::new(Box::new(|_, _| IndentBehavior::Ignore)));
        ctx.subscribe_to_model(&content, |me, _, event, ctx| {
            me.handle_buffer_event(event, ctx)
        });

        let buffer_selection = ctx.add_model(|_| BufferSelectionModel::new(content.clone()));
        // Rendered documents wrap to the viewport; code keeps its lines and scrolls sideways.
        let width_setting = match format {
            DocumentFormat::PlainText => WidthSetting::InfiniteWidth,
            DocumentFormat::Markdown | DocumentFormat::Notebook => WidthSetting::FitViewport,
        };
        let render_state = ctx.add_model(|ctx| {
            RenderState::new(styles, false, None, ctx).with_width_setting(width_setting)
        });
        ctx.subscribe_to_model(&render_state, |me, _, event, ctx| {
            if let RenderEvent::NeedsResize = event
                && me.format != DocumentFormat::PlainText
            {
                me.rebuild_layout(ctx);
            }
        });
        let selection = ctx.add_model(|ctx| {
            SelectionModel::new(
                content.clone(),
                render_state.clone(),
                buffer_selection.clone(),
                None,
                ctx,
            )
        });

        let color_map = syntax_color_map(Appearance::as_ref(ctx));
        let version = content.as_ref(ctx).buffer_version();
        let weak_content = content.downgrade();
        let syntax_tree = ctx.add_model(|_| SyntaxTreeState::new(weak_content, version, color_map));
        ctx.subscribe_to_model(&syntax_tree, |_, _, event, ctx| match event {
            DecorationStateEvent::DecorationUpdated { .. } => {
                ctx.emit(FileViewModelEvent::HighlightingUpdated)
            }
        });

        Self {
            content,
            buffer_selection,
            selection,
            render_state,
            syntax_tree,
            format,
        }
    }

    pub fn format(&self) -> DocumentFormat {
        self.format
    }

    /// Picks the highlighting language from the file name. Call before [`Self::load`].
    pub fn set_language_for_path(&mut self, path: &Path, ctx: &mut ModelContext<Self>) -> bool {
        match language_by_local_filename(path) {
            Some(language) => {
                self.set_language(language, ctx);
                true
            }
            None => false,
        }
    }

    pub fn set_language(&mut self, language: Arc<Language>, ctx: &mut ModelContext<Self>) {
        self.syntax_tree
            .update(ctx, |syntax_tree, _| syntax_tree.set_language(language));
    }

    pub fn has_highlighting(&self, ctx: &AppContext) -> bool {
        self.syntax_tree.as_ref(ctx).has_supported_highlighting()
    }

    /// Replaces the document with `text`, interpreted in the model's format.
    pub fn load(&mut self, text: &str, ctx: &mut ModelContext<Self>) {
        let state = match self.format {
            DocumentFormat::Markdown => InitialBufferState::markdown(text),
            DocumentFormat::Notebook => InitialBufferState::ipynb(text),
            DocumentFormat::PlainText => InitialBufferState::plain_text(text),
        };
        let selection = self.buffer_selection.clone();
        self.update_content(
            |mut content, ctx| {
                content.buffer().reset_undo_stack();
                content.apply_edit(
                    BufferEditAction::ReplaceWith(state),
                    EditOrigin::SystemEdit,
                    selection,
                    ctx,
                );
            },
            ctx,
        );
    }

    /// The document as plain text (Markdown is serialised back to Markdown).
    pub fn text(&self, ctx: &AppContext) -> String {
        let buffer = self.content.as_ref(ctx);
        match self.format {
            DocumentFormat::PlainText => buffer.text().into_string(),
            DocumentFormat::Markdown | DocumentFormat::Notebook => buffer.markdown_unescaped(),
        }
    }

    pub fn line_count(&self, ctx: &AppContext) -> usize {
        self.content
            .as_ref(ctx)
            .text()
            .into_string()
            .lines()
            .count()
    }

    pub fn select_at(&mut self, offset: CharOffset, ctx: &mut ModelContext<Self>) {
        self.begin_selection(offset, SelectionMode::Character, true, ctx);
    }

    pub fn select_word_at(&mut self, offset: CharOffset, ctx: &mut ModelContext<Self>) {
        self.begin_selection(
            offset,
            SelectionMode::Word(WordBoundariesPolicy::Default),
            true,
            ctx,
        );
    }

    pub fn select_line_at(&mut self, offset: CharOffset, ctx: &mut ModelContext<Self>) {
        self.begin_selection(offset, SelectionMode::Line, true, ctx);
    }

    /// Copies the selection: plain text, plus HTML for rendered documents.
    pub fn copy(&self, ctx: &mut ModelContext<Self>) {
        let clipboard = match self.format {
            DocumentFormat::PlainText => {
                let buffer = self.content.as_ref(ctx);
                warpui::clipboard::ClipboardContent::plain_text(
                    buffer
                        .selected_text_as_plain_text(self.buffer_selection.clone(), ctx)
                        .into_string(),
                )
            }
            DocumentFormat::Markdown | DocumentFormat::Notebook => {
                RichTextEditorModel::read_selected_text_as_clipboard_content(self, ctx)
            }
        };
        if !clipboard.plain_text.is_empty() {
            ctx.clipboard().write(clipboard);
        }
    }

    pub fn scroll(&self, delta: warpui::units::Pixels, ctx: &mut ModelContext<Self>) {
        self.render_state
            .update(ctx, |render_state, ctx| render_state.scroll(delta, ctx));
    }

    /// Applies new styles (theme change); re-lays out when sizes changed.
    pub fn update_styles(&mut self, styles: RichTextStyles, ctx: &mut ModelContext<Self>) {
        let color_map = syntax_color_map(Appearance::as_ref(ctx));
        self.syntax_tree
            .update(ctx, |syntax_tree, _| syntax_tree.set_color_map(color_map));
        let colors_changed = {
            let current = self.render_state.as_ref(ctx).styles();
            current.inline_code_style != styles.inline_code_style
                || current.base_text.text_color != styles.base_text.text_color
        };
        let update = self
            .render_state
            .update(ctx, |render_state, _| render_state.update_styles(styles));
        match update {
            warp_editor::render::model::StyleUpdateAction::Relayout => self.rebuild_layout(ctx),
            warp_editor::render::model::StyleUpdateAction::Repaint if colors_changed => {
                self.rebuild_layout(ctx)
            }
            _ => {}
        }
        // Re-highlight with the new colours.
        self.rehighlight(ctx);
    }

    /// Syntax colours for the visible ranges.
    pub fn text_decorations<'a>(
        &'a self,
        ranges: RangeSet<CharOffset>,
        version: Option<BufferVersion>,
        ctx: &'a AppContext,
    ) -> TextDecoration<'a> {
        TextDecoration {
            base_color_map: self
                .syntax_tree
                .as_ref(ctx)
                .highlights_in_ranges(ranges, version, ctx),
            ..Default::default()
        }
    }

    fn rehighlight(&mut self, ctx: &mut ModelContext<Self>) {
        let content = self.content.as_ref(ctx);
        let delta = content.invalidate_layout();
        let version = content.buffer_version();
        let snapshot = content.buffer_snapshot();
        self.syntax_tree.update(ctx, move |syntax_tree, ctx| {
            syntax_tree.update_internal_state_with_delta(
                &delta.precise_deltas,
                version,
                snapshot,
                ctx,
            )
        });
    }

    fn handle_buffer_event(&mut self, event: &BufferEvent, ctx: &mut ModelContext<Self>) {
        match event {
            BufferEvent::ContentChanged {
                delta,
                should_autoscroll,
                buffer_version,
                ..
            } => {
                if self.format == DocumentFormat::PlainText {
                    let snapshot = self.content.as_ref(ctx).buffer_snapshot();
                    let deltas = delta.precise_deltas.clone();
                    let version = *buffer_version;
                    self.syntax_tree.update(ctx, move |syntax_tree, ctx| {
                        syntax_tree
                            .update_internal_state_with_delta(&deltas, version, snapshot, ctx)
                    });
                }
                let (delta, version, autoscroll) =
                    (delta.clone(), *buffer_version, *should_autoscroll);
                self.render_state.update(ctx, move |render_state, _| {
                    render_state.add_pending_edit(delta, version);
                    match autoscroll {
                        ShouldAutoscroll::Yes => render_state.request_autoscroll(),
                        ShouldAutoscroll::VerticalOnly => {
                            render_state.request_vertical_autoscroll()
                        }
                        ShouldAutoscroll::No => {}
                    }
                });
            }
            BufferEvent::SelectionChanged { buffer_version, .. } => {
                let mut selections = self
                    .content
                    .as_ref(ctx)
                    .to_rendered_selection_set(self.buffer_selection.clone(), ctx);
                // Render offsets are one less than buffer offsets.
                for selection in selections.iter_mut() {
                    selection.head -= CharOffset::from(1);
                    selection.tail -= CharOffset::from(1);
                }
                let version = *buffer_version;
                // No autoscroll: in a viewer, selecting (even everything) keeps the position.
                self.render_state.update(ctx, move |render_state, ctx| {
                    render_state.update_selection(selections, version);
                    ctx.notify();
                });
            }
            _ => {}
        }
    }
}

/// Warp's mapping of syntax classes onto the theme's ANSI colours (MatterLite.md §11.1).
fn syntax_color_map(appearance: &Appearance) -> ColorMap {
    let colors = appearance.theme().terminal_colors().normal;
    ColorMap {
        keyword_color: colors.magenta.into(),
        function_color: colors.blue.into(),
        string_color: colors.green.into(),
        type_color: colors.red.into(),
        number_color: colors.green.into(),
        comment_color: colors.yellow.into(),
        property_color: colors.cyan.into(),
        tag_color: colors.red.into(),
    }
}

impl Entity for FileViewModel {
    type Event = FileViewModelEvent;
}

impl CoreEditorModel for FileViewModel {
    type T = Self;

    fn content(&self) -> &ModelHandle<Buffer> {
        &self.content
    }

    fn buffer_selection_model(&self) -> &ModelHandle<BufferSelectionModel> {
        &self.buffer_selection
    }

    fn selection_model(&self) -> &ModelHandle<SelectionModel> {
        &self.selection
    }

    fn render_state(&self) -> &ModelHandle<RenderState> {
        &self.render_state
    }

    fn validate(&self, _ctx: &impl ModelAsRef) {}

    fn active_text_style(&self) -> TextStyles {
        Default::default()
    }
}

impl RichTextEditorModel for FileViewModel {}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
