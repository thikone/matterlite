// SPDX-License-Identifier: AGPL-3.0-only

//! The rendered message, independent of any UI (MatterLite.md §12 `MessageDoc`).

use mm_model::{ChannelId, UserId};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageDoc {
    pub blocks: Vec<Block>,
    /// The message is a few emoji only: draw them large (the webapp's jumbo emoji).
    pub jumbo_emoji: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Quote(Vec<Block>),
    /// `start` is the first number of an ordered list, `None` for bullets.
    List {
        start: Option<usize>,
        items: Vec<ListItem>,
    },
    Code(CodeBlock),
    Table(Table),
    Rule,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// A task list item: checked or not.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodeBlock {
    /// As written after the fence (`rust`, `js`, `latex`, …), lower case.
    pub language: Option<String>,
    /// The code, line by line, highlighted when the language is known.
    pub lines: Vec<Vec<CodeSpan>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    pub text: String,
    pub highlight: Option<Highlight>,
}

/// Syntax classes; the UI maps them to the theme's ANSI colours (MatterLite.md §11.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Function,
    String,
    Type,
    Number,
    Comment,
    Property,
    Tag,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub alignments: Vec<Alignment>,
    pub header: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
}

/// A run of inline content with one style.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub kind: SpanKind,
}

impl Span {
    pub fn text(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
            kind: SpanKind::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Inline code.
    pub code: bool,
    /// One of my mention keys: drawn with the mention highlight.
    pub highlight: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpanKind {
    Text,
    /// A link; `text` is what is shown.
    Link(String),
    /// `@username`, `@channel`, `@here`, `@all`.
    Mention(Mention),
    /// `~channel-name`.
    Channel(ChannelMention),
    Emoji(EmojiRef),
    /// `#hashtag` (clicking searches for it).
    Hashtag,
    /// An inline image; `text` is the alt text.
    Image {
        url: String,
        width: Option<u32>,
        height: Option<u32>,
    },
    LineBreak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// Without the `@`, lower case.
    pub username: String,
    /// The user, when known.
    pub user: Option<UserId>,
    /// The user's display name in the user's name format, when known.
    pub display_name: Option<String>,
    /// `@channel`, `@all` or `@here`.
    pub special: bool,
    /// It mentions me.
    pub me: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMention {
    pub name: String,
    pub display_name: String,
    pub channel: Option<ChannelId>,
    /// The team's URL name when the channel is in another team.
    pub team: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmojiRef {
    /// A system emoji: its short name and image asset path.
    System { name: String, image: String },
    /// A custom emoji of the server, by name.
    Custom { name: String },
}

impl EmojiRef {
    pub fn name(&self) -> &str {
        match self {
            Self::System { name, .. } | Self::Custom { name } => name,
        }
    }
}

impl MessageDoc {
    /// The message as one line of plain text (History excerpts, notifications): Markdown
    /// removed, emoji as `:name:` for custom ones and as characters for system ones.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            if !out.is_empty() {
                out.push(' ');
            }
            block_text(block, &mut out);
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

fn block_text(block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(spans) | Block::Heading { spans, .. } => spans_text(spans, out),
        Block::Quote(blocks) => {
            for block in blocks {
                block_text(block, out);
                out.push(' ');
            }
        }
        Block::List { items, .. } => {
            for item in items {
                for block in &item.blocks {
                    block_text(block, out);
                    out.push(' ');
                }
            }
        }
        Block::Code(code) => {
            for line in &code.lines {
                for span in line {
                    out.push_str(&span.text);
                }
                out.push(' ');
            }
        }
        Block::Table(table) => {
            for cell in table.header.iter().chain(table.rows.iter().flatten()) {
                spans_text(cell, out);
                out.push(' ');
            }
        }
        Block::Rule => {}
    }
}

fn spans_text(spans: &[Span], out: &mut String) {
    for span in spans {
        match &span.kind {
            SpanKind::LineBreak => out.push(' '),
            SpanKind::Emoji(EmojiRef::System { name, .. }) => match mm_emoji::by_name(name) {
                Some(emoji) => out.push_str(&emoji.text),
                None => out.push_str(&span.text),
            },
            _ => out.push_str(&span.text),
        }
    }
}
