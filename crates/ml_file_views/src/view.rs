// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/notebooks/editor/view.rs` and `app/src/notebooks/file/mod.rs`
// @ 5bb7bcc6a (RichTextElement in a vertical Scrollable, mouse-to-selection mapping, the
// Rendered/Raw toggle for Markdown files); the code is new and read-only.

//! A read-only viewer for a local text file (MatterLite.md §14): Markdown and Jupyter notebooks
//! rendered (Markdown with a Raw toggle), code highlighted, other text as monospace lines. Text
//! can be selected with the mouse and copied.

use std::path::{Path, PathBuf};

use rangemap::RangeSet;
use string_offset::CharOffset;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::content::version::BufferVersion;
use warp_editor::editor::{EditorView, EmbeddedItemModel, RunnableCommandModel, TextDecoration};
use warp_editor::model::CoreEditorModel as _;
use warp_editor::render::element::{
    DisplayOptions, DisplayStateHandle, RichTextAction, RichTextElement,
};
use warp_editor::render::model::Location;
use warpui::elements::{
    Align, Border, Clipped, ConstrainedBox, Container, CrossAxisAlignment, Fill, Flex,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, ScrollStateHandle,
    Scrollable, ScrollableElement, ScrollbarWidth, Shrinkable, Text,
};
use warpui::event::ModifiersState;
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::units::Pixels;
use warpui::{
    AppContext, Element, Entity, FocusContext, ModelHandle, SingletonEntity as _, TypedActionView,
    View, ViewContext, WeakViewHandle,
};

use crate::file_type::{FileViewKind, file_view_kind};
use crate::model::{DocumentFormat, FileViewModel, FileViewModelEvent};
use crate::styles::{code_text_styles, document_text_styles};

const HEADER_HEIGHT: f32 = 34.;
const HEADER_PADDING: f32 = 12.;
/// Room between the window edge and the text.
const CONTENT_GUTTER: f32 = 16.;
const CODE_GUTTER: f32 = 8.;
const SCROLLBAR_WIDTH: ScrollbarWidth = ScrollbarWidth::Auto;

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([
        FixedBinding::new("cmdorctrl-c", FileViewAction::Copy, id!("FileView")),
        FixedBinding::new("cmdorctrl-a", FileViewAction::SelectAll, id!("FileView")),
        FixedBinding::new(
            "pageup",
            FileViewAction::Page { down: false },
            id!("FileView"),
        ),
        FixedBinding::new(
            "pagedown",
            FileViewAction::Page { down: true },
            id!("FileView"),
        ),
        FixedBinding::new("up", FileViewAction::Line { down: false }, id!("FileView")),
        FixedBinding::new("down", FileViewAction::Line { down: true }, id!("FileView")),
    ]);
}

#[derive(Clone, Debug)]
pub enum FileViewAction {
    Scroll(Pixels),
    Line {
        down: bool,
    },
    Page {
        down: bool,
    },
    SelectionStart(CharOffset),
    SelectionUpdate(CharOffset),
    SelectWord(CharOffset),
    SelectLine(CharOffset),
    SelectionEnd,
    Copy,
    SelectAll,
    /// Markdown only: switch between the rendered document and its source.
    ToggleRaw,
}

/// What the view shows.
enum Content {
    Document {
        /// The document as opened: rendered Markdown/notebook, or code/text.
        primary: ModelHandle<FileViewModel>,
        /// Markdown source, created the first time Raw is chosen.
        raw: Option<ModelHandle<FileViewModel>>,
        source: String,
        line_count: usize,
    },
    /// The file can't be shown here: why, for the placeholder.
    Unavailable(String),
}

pub struct FileView {
    path: PathBuf,
    kind: FileViewKind,
    content: Content,
    showing_raw: bool,
    selecting: bool,
    self_handle: WeakViewHandle<Self>,
    display_state: DisplayStateHandle,
    scroll_state: ScrollStateHandle,
    raw_toggle_mouse_state: MouseStateHandle,
}

impl FileView {
    /// Opens `path`. Files that can't be shown as text (images, binaries, files over the size
    /// limit, unreadable files) get a placeholder saying so.
    pub fn new(path: impl Into<PathBuf>, ctx: &mut ViewContext<Self>) -> Self {
        let path = path.into();
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let kind = file_view_kind(&path, size);
        let content = if kind.is_text() {
            match std::fs::read_to_string(&path) {
                Ok(source) => Self::load_document(&path, kind, source, ctx),
                Err(err) => Content::Unavailable(format!("Couldn't read the file: {err}")),
            }
        } else {
            Content::Unavailable(match kind {
                FileViewKind::Image => "Images open in the image viewer.".to_string(),
                _ => "This file can't be shown here. Open it with another app.".to_string(),
            })
        };

        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.update_styles(ctx);
            ctx.notify();
        });

        Self {
            path,
            kind,
            content,
            showing_raw: false,
            selecting: false,
            self_handle: ctx.handle(),
            display_state: Default::default(),
            scroll_state: Default::default(),
            raw_toggle_mouse_state: Default::default(),
        }
    }

    fn load_document(
        path: &Path,
        kind: FileViewKind,
        source: String,
        ctx: &mut ViewContext<Self>,
    ) -> Content {
        let format = match kind {
            FileViewKind::Markdown => DocumentFormat::Markdown,
            FileViewKind::Notebook => DocumentFormat::Notebook,
            _ => DocumentFormat::PlainText,
        };
        let primary = Self::add_model(format, Some(path), &source, ctx);
        Content::Document {
            primary,
            raw: None,
            line_count: source.lines().count(),
            source,
        }
    }

    fn add_model(
        format: DocumentFormat,
        language_path: Option<&Path>,
        text: &str,
        ctx: &mut ViewContext<Self>,
    ) -> ModelHandle<FileViewModel> {
        let styles = {
            let appearance = Appearance::as_ref(ctx);
            match format {
                DocumentFormat::PlainText => code_text_styles(appearance),
                DocumentFormat::Markdown | DocumentFormat::Notebook => {
                    document_text_styles(appearance)
                }
            }
        };
        let model = ctx.add_model(|ctx| FileViewModel::new(format, styles, ctx));
        model.update(ctx, |model, ctx| {
            match (format, language_path) {
                (DocumentFormat::PlainText, Some(path)) => {
                    model.set_language_for_path(path, ctx);
                }
                // Relative image links in a document resolve against its folder.
                (_, Some(path)) => model.set_document_path(Some(path.to_path_buf()), ctx),
                (_, None) => {}
            }
            model.load(text, ctx);
        });
        ctx.subscribe_to_model(&model, |_, _, event, ctx| match event {
            FileViewModelEvent::HighlightingUpdated => ctx.notify(),
        });
        // Re-render when the layout or viewport changes.
        let render_state = model.as_ref(ctx).render_state().clone();
        ctx.observe(&render_state, |_, _, ctx| ctx.notify());
        model
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> FileViewKind {
        self.kind
    }

    /// The model being shown (the Raw one when Raw is on).
    pub fn active_model(&self) -> Option<&ModelHandle<FileViewModel>> {
        match &self.content {
            Content::Document { primary, raw, .. } => Some(match raw {
                Some(raw) if self.showing_raw => raw,
                _ => primary,
            }),
            Content::Unavailable(_) => None,
        }
    }

    pub fn is_showing_raw(&self) -> bool {
        self.showing_raw
    }

    fn toggle_raw(&mut self, ctx: &mut ViewContext<Self>) {
        if self.kind != FileViewKind::Markdown {
            return;
        }
        if let Content::Document { raw, source, .. } = &mut self.content
            && raw.is_none()
        {
            let source = source.clone();
            let path = self.path.clone();
            let model = Self::add_model(DocumentFormat::PlainText, Some(&path), &source, ctx);
            if let Content::Document { raw, .. } = &mut self.content {
                *raw = Some(model);
            }
        }
        self.showing_raw = !self.showing_raw;
        ctx.notify();
    }

    fn update_styles(&mut self, ctx: &mut ViewContext<Self>) {
        let Content::Document { primary, raw, .. } = &self.content else {
            return;
        };
        let models: Vec<_> = std::iter::once(primary.clone())
            .chain(raw.clone())
            .collect();
        for model in models {
            let styles = {
                let appearance = Appearance::as_ref(ctx);
                match model.as_ref(ctx).format() {
                    DocumentFormat::PlainText => code_text_styles(appearance),
                    _ => document_text_styles(appearance),
                }
            };
            model.update(ctx, |model, ctx| model.update_styles(styles, ctx));
        }
    }

    fn with_active_model(
        &mut self,
        ctx: &mut ViewContext<Self>,
        update: impl FnOnce(&mut FileViewModel, &mut warpui::ModelContext<FileViewModel>),
    ) {
        if let Some(model) = self.active_model().cloned() {
            model.update(ctx, update);
            ctx.notify();
        }
    }

    fn line_height(&self, app: &AppContext) -> f32 {
        self.active_model()
            .map(|model| {
                model
                    .as_ref(app)
                    .render_state()
                    .as_ref(app)
                    .styles()
                    .base_text
                    .line_height()
                    .as_f32()
            })
            .unwrap_or(20.)
    }

    fn render_header(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());

        let mut details = vec![
            match self.kind {
                FileViewKind::Markdown => "Markdown",
                FileViewKind::Notebook => "Jupyter notebook",
                FileViewKind::Code => "Code",
                FileViewKind::Text => "Text",
                FileViewKind::Image => "Image",
                FileViewKind::External => "Other",
            }
            .to_string(),
        ];
        if let Content::Document {
            line_count,
            primary,
            ..
        } = &self.content
        {
            // A notebook's lines are JSON lines; not worth showing.
            if self.kind != FileViewKind::Notebook {
                details.push(format!(
                    "{line_count} line{}",
                    if *line_count == 1 { "" } else { "s" }
                ));
            }
            if self.kind == FileViewKind::Code && !primary.as_ref(app).has_highlighting(app) {
                details.push("no highlighting".to_string());
            }
        }
        details.push("read-only".to_string());

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.,
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Text::new_inline(name, font, size + 1.)
                                .with_color(theme.main_text_color(theme.background()).into_solid())
                                .finish(),
                        )
                        .with_child(
                            Container::new(
                                Text::new_inline(details.join(" · "), font, size - 1.)
                                    .with_color(
                                        theme.sub_text_color(theme.background()).into_solid(),
                                    )
                                    .finish(),
                            )
                            .with_margin_left(10.)
                            .finish(),
                        )
                        .finish(),
                )
                .finish(),
            );
        if self.kind == FileViewKind::Markdown && matches!(self.content, Content::Document { .. }) {
            let label = if self.showing_raw { "Rendered" } else { "Raw" };
            row.add_child(
                appearance
                    .ui_builder()
                    .button(
                        warpui::ui_components::button::ButtonVariant::Secondary,
                        self.raw_toggle_mouse_state.clone(),
                    )
                    .with_text_label(format!("Show {label}"))
                    .with_style(UiComponentStyles {
                        font_size: Some(size - 1.),
                        ..Default::default()
                    })
                    .build()
                    .on_click(|ctx, _, _| ctx.dispatch_typed_action(FileViewAction::ToggleRaw))
                    .with_cursor(Cursor::PointingHand)
                    .finish(),
            );
        }

        Container::new(
            ConstrainedBox::new(row.finish())
                .with_height(HEADER_HEIGHT)
                .finish(),
        )
        .with_padding_left(HEADER_PADDING)
        .with_padding_right(HEADER_PADDING)
        .with_border(Border::bottom(1.).with_border_fill(internal_colors::fg_overlay_3(theme)))
        .finish()
    }

    fn render_body(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let Some(model) = self.active_model() else {
            let message = match &self.content {
                Content::Unavailable(message) => message.clone(),
                Content::Document { .. } => String::new(),
            };
            return Align::new(
                Text::new(
                    message,
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.hint_text_color(theme.background()).into_solid())
                .finish(),
            )
            .finish();
        };
        let is_code = model.as_ref(app).format() == DocumentFormat::PlainText;
        let gutter = if is_code { CODE_GUTTER } else { CONTENT_GUTTER };
        let focused = self
            .self_handle
            .upgrade(app)
            .is_some_and(|handle| handle.is_focused(app));

        let display_options = DisplayOptions {
            editable: false,
            focused,
            blink_cursors: false,
            // `DisplayOptions::default()` turns this on.
            debug_bounds: false,
            left_gutter: gutter,
            right_gutter: gutter,
            ..Default::default()
        };
        let rich_text = RichTextElement::<Self>::new(
            model.as_ref(app).render_state().clone(),
            self.self_handle.clone(),
            display_options,
            self.display_state.clone(),
            None,
            Vec::new(),
        )
        .finish_scrollable();

        Clipped::new(
            Scrollable::vertical(
                self.scroll_state.clone(),
                rich_text,
                SCROLLBAR_WIDTH,
                theme.disabled_text_color(theme.background()).into(),
                theme.main_text_color(theme.background()).into(),
                Fill::None,
            )
            .finish(),
        )
        .finish()
    }
}

impl Entity for FileView {
    type Event = ();
}

impl View for FileView {
    fn ui_name() -> &'static str {
        "FileView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(self.render_header(appearance, app))
            .with_child(Shrinkable::new(1., self.render_body(appearance, app)).finish())
            .finish()
    }

    fn on_focus(&mut self, _: &FocusContext, ctx: &mut ViewContext<Self>) {
        ctx.notify();
    }
}

impl TypedActionView for FileView {
    type Action = FileViewAction;

    fn handle_action(&mut self, action: &FileViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            FileViewAction::Scroll(delta) => {
                let delta = *delta;
                self.with_active_model(ctx, |model, ctx| model.scroll(delta, ctx));
            }
            FileViewAction::Line { down } => {
                let step = self.line_height(ctx) * 3.;
                // Wheel convention: a positive delta scrolls up.
                let delta = Pixels::new(if *down { -step } else { step });
                self.with_active_model(ctx, |model, ctx| model.scroll(delta, ctx));
            }
            FileViewAction::Page { down } => {
                let step = self.line_height(ctx) * 20.;
                // Wheel convention: a positive delta scrolls up.
                let delta = Pixels::new(if *down { -step } else { step });
                self.with_active_model(ctx, |model, ctx| model.scroll(delta, ctx));
            }
            FileViewAction::SelectionStart(offset) => {
                ctx.focus_self();
                self.selecting = true;
                let offset = *offset;
                self.with_active_model(ctx, |model, ctx| model.select_at(offset, ctx));
            }
            FileViewAction::SelectionUpdate(offset) => {
                let offset = *offset;
                self.with_active_model(ctx, |model, ctx| {
                    model.update_pending_selection(offset, ctx)
                });
            }
            FileViewAction::SelectWord(offset) => {
                ctx.focus_self();
                self.selecting = true;
                let offset = *offset;
                self.with_active_model(ctx, |model, ctx| model.select_word_at(offset, ctx));
            }
            FileViewAction::SelectLine(offset) => {
                ctx.focus_self();
                self.selecting = true;
                let offset = *offset;
                self.with_active_model(ctx, |model, ctx| model.select_line_at(offset, ctx));
            }
            FileViewAction::SelectionEnd => {
                self.selecting = false;
                self.with_active_model(ctx, |model, ctx| model.end_selection(ctx));
            }
            FileViewAction::Copy => self.with_active_model(ctx, |model, ctx| model.copy(ctx)),
            FileViewAction::SelectAll => {
                self.with_active_model(ctx, |model, ctx| model.select_all(ctx))
            }
            FileViewAction::ToggleRaw => self.toggle_raw(ctx),
        }
    }
}

impl EditorView for FileView {
    type RichTextAction = FileViewAction;

    fn runnable_command_at<'a>(
        &self,
        _block_offset: CharOffset,
        _ctx: &'a AppContext,
    ) -> Option<&'a dyn RunnableCommandModel> {
        None
    }

    fn embedded_item_at<'a>(
        &self,
        _block_offset: CharOffset,
        _ctx: &'a AppContext,
    ) -> Option<&'a dyn EmbeddedItemModel> {
        None
    }

    fn text_decorations<'a>(
        &'a self,
        viewport_ranges: RangeSet<CharOffset>,
        content_version: Option<BufferVersion>,
        ctx: &'a AppContext,
    ) -> TextDecoration<'a> {
        match self.active_model() {
            Some(model) => {
                model
                    .as_ref(ctx)
                    .text_decorations(viewport_ranges, content_version, ctx)
            }
            None => TextDecoration::default(),
        }
    }
}

impl RichTextAction<FileView> for FileViewAction {
    fn scroll(delta: Pixels, axis: warpui::elements::Axis) -> Option<Self> {
        match axis {
            warpui::elements::Axis::Vertical => Some(FileViewAction::Scroll(delta)),
            warpui::elements::Axis::Horizontal => None,
        }
    }

    fn user_typed(_: String, _: &WeakViewHandle<FileView>, _: &AppContext) -> Option<Self> {
        None
    }

    fn vim_user_typed(_: String, _: &WeakViewHandle<FileView>, _: &AppContext) -> Option<Self> {
        None
    }

    fn left_mouse_down(
        location: Location,
        modifiers: ModifiersState,
        click_count: u32,
        is_first_mouse: bool,
        _: &WeakViewHandle<FileView>,
        _: &AppContext,
    ) -> Option<Self> {
        // The click that activates the window has no matching mouse-up.
        if is_first_mouse {
            return None;
        }
        // Render offsets are one less than buffer offsets (as in Warp's editors).
        let Location::Text { char_offset, .. } = location else {
            return None;
        };
        let offset = char_offset + 1;
        match click_count {
            1 if modifiers.shift => Some(FileViewAction::SelectionUpdate(offset)),
            1 => Some(FileViewAction::SelectionStart(offset)),
            2 => Some(FileViewAction::SelectWord(offset)),
            3 => Some(FileViewAction::SelectLine(offset)),
            _ => None,
        }
    }

    fn left_mouse_dragged(
        location: Location,
        _cmd: bool,
        _shift: bool,
        view: &WeakViewHandle<FileView>,
        ctx: &AppContext,
    ) -> Option<Self> {
        let selecting = view.upgrade(ctx)?.as_ref(ctx).selecting;
        match location {
            Location::Text { char_offset, .. } if selecting => {
                Some(FileViewAction::SelectionUpdate(char_offset + 1))
            }
            _ => None,
        }
    }

    fn left_mouse_up(
        _: Location,
        _cmd: bool,
        _shift: bool,
        _: &WeakViewHandle<FileView>,
        _: &AppContext,
    ) -> Vec<Self> {
        vec![FileViewAction::SelectionEnd]
    }

    fn mouse_hovered(
        _: Option<Location>,
        _: &WeakViewHandle<FileView>,
        _cmd: bool,
        _is_covered: bool,
        _: &AppContext,
    ) -> Option<Self> {
        None
    }

    fn task_list_clicked(
        _: CharOffset,
        _: &WeakViewHandle<FileView>,
        _: &AppContext,
    ) -> Option<Self> {
        None
    }

    fn middle_mouse_down(_: &AppContext) -> Option<Self> {
        None
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
