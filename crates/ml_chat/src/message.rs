// SPDX-License-Identifier: AGPL-3.0-only

//! A rendered message ([`MessageDoc`]) as WarpUI elements (MatterLite.md §12): a `Text` with
//! styled character ranges per paragraph, the block layout around them, code in the
//! monospace font with the theme's ANSI colours, emoji as text (the system's colour emoji
//! font) or images (custom emoji, jumbo emoji).

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use mm_render::{
    Alignment, Block, CodeBlock, EmojiRef, Highlight as Syntax, MessageDoc, Span, SpanKind, Table,
};
use warpui::Element;
use warpui::assets::asset_cache::AssetSource;
use warpui::color::ColorU;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Flex, Highlight,
    HighlightedRange, Image, MainAxisSize, ParentElement, PartialClickableElement as _, Radius,
    Rect, Shrinkable, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Style as FontStyle, Weight};
use warpui::image_cache::CacheOption;
use warpui::text_layout::TextStyle;

/// Where a click on a message goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    /// `@name` (a user or a channel-wide mention).
    Mention(String),
    /// `~name`.
    Channel(String),
    Hashtag(String),
}

/// Fonts and colours of a message.
#[derive(Clone, Debug)]
pub struct Palette {
    pub font: FamilyId,
    pub mono: FamilyId,
    pub size: f32,
    pub text: ColorU,
    pub muted: ColorU,
    pub link: ColorU,
    pub mention_bg: ColorU,
    pub mention_text: ColorU,
    pub code_bg: ColorU,
    pub border: ColorU,
    pub syntax: SyntaxColors,
}

#[derive(Clone, Debug)]
pub struct SyntaxColors {
    pub keyword: ColorU,
    pub function: ColorU,
    pub string: ColorU,
    pub kind: ColorU,
    pub number: ColorU,
    pub comment: ColorU,
    pub property: ColorU,
    pub tag: ColorU,
}

impl Palette {
    /// Messages on the window's background in the current theme: text colours, the chat
    /// palette's links and mention highlight, code in the monospace font with the ANSI colours.
    pub fn of(appearance: &warp_core::ui::appearance::Appearance) -> Self {
        let theme = appearance.theme();
        let chat = theme.chat();
        let background = theme.background();
        let text = theme.main_text_color(background).into_solid();
        let ansi = &theme.terminal_colors().normal;
        let bright = &theme.terminal_colors().bright;
        Self {
            font: appearance.ui_font_family(),
            mono: appearance.monospace_font_family(),
            size: appearance.ui_font_size() + 1.,
            text,
            muted: theme.sub_text_color(background).into_solid(),
            link: chat.link,
            mention_bg: chat.mention_highlight_bg,
            mention_text: chat.mention_highlight_link,
            code_bg: crate::chats_view::fade(text, 0.07),
            border: crate::chats_view::fade(text, 0.16),
            syntax: SyntaxColors {
                keyword: ansi.magenta.into(),
                function: ansi.blue.into(),
                string: ansi.green.into(),
                kind: ansi.yellow.into(),
                number: ansi.red.into(),
                comment: bright.black.into(),
                property: ansi.cyan.into(),
                tag: ansi.red.into(),
            },
        }
    }
}

impl SyntaxColors {
    fn of(&self, class: Syntax) -> ColorU {
        match class {
            Syntax::Keyword => self.keyword,
            Syntax::Function => self.function,
            Syntax::String => self.string,
            Syntax::Type => self.kind,
            Syntax::Number => self.number,
            Syntax::Comment => self.comment,
            Syntax::Property => self.property,
            Syntax::Tag => self.tag,
        }
    }
}

/// Images of custom emoji that are on disk, by name.
pub type CustomEmoji = HashMap<String, PathBuf>;

/// What clicks on a message do.
pub type OnClick = std::rc::Rc<dyn Fn(Target, &mut warpui::EventContext)>;

const JUMBO_SIZE: f32 = 32.;
const BLOCK_SPACING: f32 = 6.;
const LIST_INDENT: f32 = 20.;

pub struct MessageRenderer<'a> {
    pub palette: &'a Palette,
    pub custom_emoji: &'a CustomEmoji,
    pub on_click: OnClick,
    /// System messages: the text in the muted colour.
    pub muted: bool,
}

impl MessageRenderer<'_> {
    pub fn render(&self, doc: &MessageDoc) -> Box<dyn Element> {
        if doc.jumbo_emoji
            && let [Block::Paragraph(spans)] = doc.blocks.as_slice()
        {
            return self.jumbo(spans);
        }
        self.blocks(&doc.blocks)
    }

    fn blocks(&self, blocks: &[Block]) -> Box<dyn Element> {
        if let [block] = blocks {
            return self.block(block);
        }
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(BLOCK_SPACING)
            .with_children(blocks.iter().map(|block| self.block(block)))
            .finish()
    }

    fn block(&self, block: &Block) -> Box<dyn Element> {
        let palette = self.palette;
        match block {
            Block::Paragraph(spans) => self.inline(spans, palette.size, false),
            Block::Heading { level, spans } => {
                let scale = match level {
                    1 => 1.5,
                    2 => 1.3,
                    3 => 1.15,
                    _ => 1.,
                };
                self.inline(spans, palette.size * scale, true)
            }
            Block::Quote(blocks) => Container::new(self.blocks(blocks))
                .with_padding_left(10.)
                .with_border(Border::left(3.).with_border_fill(palette.border))
                .finish(),
            Block::List { start, items } => {
                let mut column = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(2.);
                for (number, item) in items.iter().enumerate() {
                    let marker = match (item.task, start) {
                        (Some(done), _) => checkbox(done, palette),
                        (None, Some(start)) => Text::new_inline(
                            format!("{}.", start + number),
                            palette.font,
                            palette.size,
                        )
                        .with_color(palette.muted)
                        .finish(),
                        (None, None) => Text::new_inline("•", palette.font, palette.size)
                            .with_color(palette.muted)
                            .finish(),
                    };
                    column.add_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_child(fixed_width(LIST_INDENT, marker))
                            .with_child(Shrinkable::new(1., self.blocks(&item.blocks)).finish())
                            .finish(),
                    );
                }
                column.finish()
            }
            Block::Code(code) => self.code(code),
            Block::Table(table) => self.table(table),
            Block::Rule => {
                ConstrainedBox::new(Rect::new().with_background_color(palette.border).finish())
                    .with_height(1.)
                    .finish()
            }
        }
    }

    fn code(&self, code: &CodeBlock) -> Box<dyn Element> {
        let palette = self.palette;
        let mut text = String::new();
        let mut ranges: Vec<HighlightedRange> = Vec::new();
        let mut chars = 0;
        for (number, line) in code.lines.iter().enumerate() {
            if number > 0 {
                text.push('\n');
                chars += 1;
            }
            for span in line {
                let length = span.text.chars().count();
                if let Some(class) = span.highlight {
                    ranges.push(HighlightedRange {
                        highlight: Highlight::new().with_foreground_color(palette.syntax.of(class)),
                        highlight_indices: (chars..chars + length).collect(),
                    });
                }
                text.push_str(&span.text);
                chars += length;
            }
        }
        Container::new(
            Text::new(text, palette.mono, palette.size - 1.)
                .with_color(palette.text)
                .with_highlights(ranges)
                .finish(),
        )
        .with_uniform_padding(8.)
        .with_background_color(palette.code_bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    }

    fn table(&self, table: &Table) -> Box<dyn Element> {
        let palette = self.palette;
        let row = |cells: &[Vec<Span>], header: bool| {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
            for (column, cell) in cells.iter().enumerate() {
                let alignment = table.alignments.get(column).copied().unwrap_or_default();
                let content = self.inline(cell, palette.size, header);
                let content = match alignment {
                    Alignment::Center => warpui::elements::Align::new(content).finish(),
                    Alignment::Right => warpui::elements::Align::new(content).right().finish(),
                    Alignment::Left | Alignment::None => content,
                };
                row.add_child(
                    Shrinkable::new(
                        1.,
                        Container::new(content)
                            .with_horizontal_padding(8.)
                            .with_vertical_padding(4.)
                            .with_border(Border::all(1.).with_border_fill(palette.border))
                            .finish(),
                    )
                    .finish(),
                );
            }
            row.finish()
        };
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        column.add_child(row(&table.header, true));
        for cells in &table.rows {
            column.add_child(row(cells, false));
        }
        column.finish()
    }

    fn jumbo(&self, spans: &[Span]) -> Box<dyn Element> {
        let mut row = Flex::row().with_spacing(4.);
        for span in spans {
            if let SpanKind::Emoji(emoji) = &span.kind {
                row.add_child(match self.emoji_image(emoji) {
                    Some(image) => ConstrainedBox::new(image)
                        .with_width(JUMBO_SIZE)
                        .with_height(JUMBO_SIZE)
                        .finish(),
                    None => Text::new_inline(span.text.clone(), self.palette.font, JUMBO_SIZE)
                        .with_color(self.palette.text)
                        .finish(),
                });
            }
        }
        row.finish()
    }

    fn emoji_image(&self, emoji: &EmojiRef) -> Option<Box<dyn Element>> {
        emoji_image(emoji, self.custom_emoji)
    }

    /// A run of inline spans: one `Text` with styled ranges; a wrap of words and images when
    /// it holds custom emoji.
    fn inline(&self, spans: &[Span], size: f32, bold: bool) -> Box<dyn Element> {
        if !spans
            .iter()
            .any(|span| matches!(span.kind, SpanKind::Emoji(_)))
        {
            return self.text(spans, size, bold);
        }
        // Emoji are images: each line becomes a wrap of words and images.
        let lines: Vec<&[Span]> = spans
            .split(|span| span.kind == SpanKind::LineBreak)
            .collect();
        if let [line] = lines.as_slice() {
            return self.wrap_line(line, size, bold);
        }
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_children(lines.into_iter().map(|line| {
                if line.is_empty() {
                    self.text(&[Span::text(" ", Default::default())], size, bold)
                } else {
                    self.wrap_line(line, size, bold)
                }
            }))
            .finish()
    }

    /// Words (with their trailing spaces) as separate texts, so emoji images can sit between
    /// them.
    fn wrap_line(&self, spans: &[Span], size: f32, bold: bool) -> Box<dyn Element> {
        let mut wrap = Wrap::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        let mut words: Vec<Span> = Vec::new();
        let flush = |words: &mut Vec<Span>, wrap: &mut Wrap| {
            if !words.is_empty() {
                wrap.add_child(self.text(words, size, bold));
                words.clear();
            }
        };
        for span in spans {
            if let SpanKind::Emoji(emoji) = &span.kind
                && let Some(image) = self.emoji_image(emoji)
            {
                flush(&mut words, &mut wrap);
                let side = (size * 1.3).round();
                wrap.add_child(
                    Container::new(
                        ConstrainedBox::new(image)
                            .with_width(side)
                            .with_height(side)
                            .finish(),
                    )
                    .with_horizontal_padding(1.)
                    .finish(),
                );
                continue;
            }
            let mut rest = span.text.as_str();
            while !rest.is_empty() {
                let end = rest
                    .find(char::is_whitespace)
                    .map(|space| {
                        space
                            + rest[space..]
                                .find(|c: char| !c.is_whitespace())
                                .unwrap_or(rest.len() - space)
                    })
                    .unwrap_or(rest.len());
                words.push(Span {
                    text: rest[..end].to_owned(),
                    ..span.clone()
                });
                if rest[..end].ends_with(char::is_whitespace) {
                    flush(&mut words, &mut wrap);
                }
                rest = &rest[end..];
            }
        }
        flush(&mut words, &mut wrap);
        wrap.finish()
    }

    fn text(&self, spans: &[Span], size: f32, bold: bool) -> Box<dyn Element> {
        let palette = self.palette;
        let mut text = String::new();
        let mut ranges: Vec<HighlightedRange> = Vec::new();
        let mut targets: Vec<(Range<usize>, Target)> = Vec::new();
        let mut chars = 0;
        for span in spans {
            let shown = match &span.kind {
                SpanKind::LineBreak => "\n".to_owned(),
                SpanKind::Emoji(EmojiRef::System { name, .. }) => mm_emoji::by_name(name)
                    .map_or_else(|| span.text.clone(), |emoji| emoji.text.clone()),
                SpanKind::Image { url, .. } => {
                    if span.text.is_empty() {
                        url.clone()
                    } else {
                        span.text.clone()
                    }
                }
                _ => span.text.clone(),
            };
            let length = shown.chars().count();
            let range = chars..chars + length;
            let style = span.style;
            let mut properties = Properties::default();
            if style.bold || bold {
                properties.weight = Weight::Bold;
            }
            if style.italic {
                properties.style = FontStyle::Italic;
            }
            let mut text_style = TextStyle::new();
            let mut styled = properties != Properties::default();
            let target = match &span.kind {
                SpanKind::Link(url) | SpanKind::Image { url, .. } => Some(Target::Url(url.clone())),
                SpanKind::Mention(mention) => {
                    if mention.me {
                        text_style = text_style
                            .with_background_color(palette.mention_bg)
                            .with_foreground_color(palette.mention_text);
                        styled = true;
                    }
                    (mention.user.is_some() || mention.special)
                        .then(|| Target::Mention(span.text.trim_start_matches('@').to_owned()))
                }
                SpanKind::Channel(_) => Some(Target::Channel(
                    span.text.trim_start_matches('~').to_owned(),
                )),
                SpanKind::Hashtag => Some(Target::Hashtag(span.text.clone())),
                _ => None,
            };
            if target.is_some() && text_style.foreground_color.is_none() {
                text_style = text_style.with_foreground_color(palette.link);
                styled = true;
            }
            if style.code {
                text_style = text_style
                    .with_background_color(palette.code_bg)
                    .with_foreground_color(palette.syntax.string);
                styled = true;
            }
            if style.highlight {
                text_style = text_style.with_background_color(palette.mention_bg);
                if target.is_none() {
                    text_style = text_style.with_foreground_color(palette.mention_text);
                }
                styled = true;
            }
            if style.strike {
                text_style = text_style.with_show_strikethrough(true);
                styled = true;
            }
            if styled && length > 0 {
                ranges.push(HighlightedRange {
                    highlight: Highlight::new()
                        .with_properties(properties)
                        .with_text_style(text_style),
                    highlight_indices: range.clone().collect(),
                });
            }
            if let Some(target) = target
                && length > 0
            {
                targets.push((range, target));
            }
            text.push_str(&shown);
            chars += length;
        }
        let mut element = Text::new(text, palette.font, size)
            .with_color(if self.muted {
                palette.muted
            } else {
                palette.text
            })
            .with_highlights(ranges);
        for (range, target) in targets {
            let on_click = self.on_click.clone();
            element = element
                .with_clickable_char_range(range, move |_, ctx, _| on_click(target.clone(), ctx));
        }
        element.finish()
    }
}

/// `child` in a column exactly `width` wide (a `ConstrainedBox` gives a narrower child its
/// own width).
pub fn fixed_width(width: f32, child: Box<dyn Element>) -> Box<dyn Element> {
    warpui::elements::Stack::new()
        .with_child(
            ConstrainedBox::new(warpui::elements::Empty::new().finish())
                .with_width(width)
                .with_height(1.)
                .finish(),
        )
        .with_child(child)
        .finish()
}

/// A task list box, checked or not.
fn checkbox(done: bool, palette: &Palette) -> Box<dyn Element> {
    let side = (palette.size * 0.95).round();
    let mut container = Container::new(if done {
        ConstrainedBox::new(
            warp_core::ui::icons::Icon::Check
                .to_warpui_icon(warp_core::ui::theme::Fill::Solid(ColorU::white()))
                .finish(),
        )
        .with_width(side - 4.)
        .with_height(side - 4.)
        .finish()
    } else {
        warpui::elements::Empty::new().finish()
    })
    .with_uniform_padding(1.)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
    .with_border(Border::all(1.).with_border_color(if done {
        palette.link
    } else {
        palette.muted
    }));
    if done {
        container = container.with_background_color(palette.link);
    }
    Container::new(
        ConstrainedBox::new(container.finish())
            .with_width(side)
            .with_height(side)
            .finish(),
    )
    .with_padding_top(3.)
    .finish()
}

/// The image of an emoji (bundled for system emoji, from disk for custom ones).
pub fn emoji_image(emoji: &EmojiRef, custom: &CustomEmoji) -> Option<Box<dyn Element>> {
    let source = match emoji {
        EmojiRef::System { image, .. } => AssetSource::Bundled {
            path: intern(image),
        },
        EmojiRef::Custom { name } => AssetSource::LocalFile {
            path: custom.get(name)?.to_string_lossy().into_owned(),
            content_version: None,
        },
    };
    Some(Image::new(source, CacheOption::BySize).finish())
}

/// Bundled assets are named by `&'static str`; emoji paths are interned once each (at most
/// one per emoji in the set).
fn intern(path: &str) -> &'static str {
    static PATHS: LazyLock<Mutex<HashMap<String, &'static str>>> = LazyLock::new(Default::default);
    let mut paths = PATHS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(path) = paths.get(path) {
        return path;
    }
    let leaked: &'static str = Box::leak(path.to_owned().into_boxed_str());
    paths.insert(path.to_owned(), leaked);
    leaked
}
