// SPDX-License-Identifier: AGPL-3.0-only

//! Markdown to blocks with comrak, configured like the webapp's `marked`
//! (WebApp.md §11.1): GFM tables, strikethrough, task lists and autolinks; raw HTML shown as
//! text; single newlines are line breaks; links only with safe schemes; the image size
//! syntax `![alt](url =WxH)`.

use std::sync::LazyLock;

use comrak::nodes::{ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use regex::Regex;

use crate::Context;
use crate::doc::{Alignment, Block, CodeBlock, ListItem, Span, SpanKind, Style, Table};
use crate::highlight::highlight;
use crate::text;

/// Schemes links may use (`getAutolinkedUrlSchemes`), besides the server's
/// `CustomUrlSchemes`.
const SAFE_SCHEMES: &[&str] = &["http", "https", "ftp", "mailto", "tel", "mattermost"];

/// `![alt](url =WxH)` (also `=W`, `=Wx`, `=xH`): comrak doesn't know it, so the size moves
/// into the title, where [`image_size`] reads it back.
static IMAGE_SIZE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!\[[^\]]*\]\(\s*[^\s)]+)\s+=(\d*)(?:x(\d*))?\s*\)").expect("valid regex")
});

pub(crate) fn render(message: &str, context: &Context) -> Vec<Block> {
    let message = IMAGE_SIZE.replace_all(message, "${1} \"=${2}x${3}\")");
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    let arena = Arena::new();
    let root = parse_document(&arena, &message, &options);
    blocks(root, context)
}

type Node<'a> = comrak::nodes::Node<'a>;

fn blocks<'a>(node: Node<'a>, context: &Context) -> Vec<Block> {
    let mut out = Vec::new();
    for child in node.children() {
        let value = child.data().value.clone();
        match value {
            NodeValue::Paragraph => out.push(Block::Paragraph(inlines(child, context))),
            NodeValue::Heading(heading) => out.push(Block::Heading {
                level: heading.level,
                spans: inlines(child, context),
            }),
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                out.push(Block::Quote(blocks(child, context)))
            }
            NodeValue::List(list) => {
                let items = child
                    .children()
                    .map(|item| {
                        let task = match &item.data().value {
                            NodeValue::TaskItem(task) => Some(task.symbol.is_some()),
                            _ => None,
                        };
                        ListItem {
                            task,
                            blocks: blocks(item, context),
                        }
                    })
                    .collect();
                out.push(Block::List {
                    start: (list.list_type == ListType::Ordered).then_some(list.start),
                    items,
                });
            }
            NodeValue::CodeBlock(code) => {
                let language = code
                    .info
                    .split_whitespace()
                    .next()
                    .map(str::to_lowercase)
                    .filter(|language| !language.is_empty());
                let highlighted = language
                    .as_deref()
                    .filter(|language| !matches!(*language, "latex" | "tex" | "text" | "plain"));
                out.push(Block::Code(CodeBlock {
                    lines: highlight(&code.literal, highlighted),
                    language,
                }));
            }
            NodeValue::HtmlBlock(html) => out.push(Block::Paragraph(vec![Span::text(
                html.literal.trim_end(),
                Style::default(),
            )])),
            NodeValue::ThematicBreak => out.push(Block::Rule),
            NodeValue::Table(table) => {
                let mut header = Vec::new();
                let mut rows = Vec::new();
                for row in child.children() {
                    let is_header = matches!(row.data().value, NodeValue::TableRow(true));
                    let cells: Vec<Vec<Span>> =
                        row.children().map(|cell| inlines(cell, context)).collect();
                    if is_header {
                        header = cells;
                    } else {
                        rows.push(cells);
                    }
                }
                out.push(Block::Table(Table {
                    alignments: table.alignments.iter().map(alignment).collect(),
                    header,
                    rows,
                }));
            }
            // Front matter, footnotes and other extensions aren't enabled; anything else
            // contributes its content.
            _ => out.extend(blocks(child, context)),
        }
    }
    out
}

fn alignment(alignment: &TableAlignment) -> Alignment {
    match alignment {
        TableAlignment::Left => Alignment::Left,
        TableAlignment::Center => Alignment::Center,
        TableAlignment::Right => Alignment::Right,
        TableAlignment::None => Alignment::None,
    }
}

fn inlines<'a>(node: Node<'a>, context: &Context) -> Vec<Span> {
    let mut out = Vec::new();
    inline_children(node, Style::default(), None, context, &mut out);
    // Merge neighbouring plain text with the same style (comrak splits text at `[`, `!`, …).
    let mut merged: Vec<Span> = Vec::with_capacity(out.len());
    for span in out {
        match merged.last_mut() {
            Some(last)
                if last.kind == SpanKind::Text
                    && span.kind == SpanKind::Text
                    && last.style == span.style =>
            {
                last.text.push_str(&span.text)
            }
            _ => merged.push(span),
        }
    }
    merged
}

fn inline_children<'a>(
    node: Node<'a>,
    style: Style,
    link: Option<&str>,
    context: &Context,
    out: &mut Vec<Span>,
) {
    for child in node.children() {
        let value = child.data().value.clone();
        match value {
            NodeValue::Text(text) => match link {
                Some(url) => out.push(Span {
                    text: text.to_string(),
                    style,
                    kind: SpanKind::Link(url.to_owned()),
                }),
                None => text::format(&text, style, context, out),
            },
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(Span {
                text: "\n".to_owned(),
                style,
                kind: SpanKind::LineBreak,
            }),
            NodeValue::Code(code) => {
                let style = Style {
                    code: true,
                    ..style
                };
                out.push(match link {
                    Some(url) => Span {
                        text: code.literal,
                        style,
                        kind: SpanKind::Link(url.to_owned()),
                    },
                    None => Span::text(code.literal, style),
                });
            }
            NodeValue::HtmlInline(html) | NodeValue::Raw(html) => out.push(Span::text(html, style)),
            NodeValue::Emph => inline_children(
                child,
                Style {
                    italic: true,
                    ..style
                },
                link,
                context,
                out,
            ),
            NodeValue::Strong => inline_children(
                child,
                Style {
                    bold: true,
                    ..style
                },
                link,
                context,
                out,
            ),
            NodeValue::Strikethrough => inline_children(
                child,
                Style {
                    strike: true,
                    ..style
                },
                link,
                context,
                out,
            ),
            NodeValue::Link(target) => match safe_url(&target.url, context) {
                Some(url) => inline_children(child, style, Some(&url), context, out),
                // An unsafe link shows its text only.
                None => inline_children(child, style, None, context, out),
            },
            NodeValue::Image(target) => {
                let mut alt = Vec::new();
                inline_children(child, style, None, context, &mut alt);
                let alt: String = alt.into_iter().map(|span| span.text).collect();
                let (width, height) = image_size(&target.title);
                match safe_url(&target.url, context) {
                    Some(url) => out.push(Span {
                        text: alt,
                        style,
                        kind: SpanKind::Image { url, width, height },
                    }),
                    None => out.push(Span::text(alt, style)),
                }
            }
            _ => inline_children(child, style, link, context, out),
        }
    }
}

/// The size moved into the title by [`IMAGE_SIZE`].
fn image_size(title: &str) -> (Option<u32>, Option<u32>) {
    let Some(size) = title.strip_prefix('=') else {
        return (None, None);
    };
    let (width, height) = size.split_once('x').unwrap_or((size, ""));
    (width.parse().ok(), height.parse().ok())
}

/// The URL to open, or `None` for schemes that aren't allowed (`javascript:`, `data:`, …).
/// A URL without a scheme gets `http://`, as in the webapp.
fn safe_url(url: &str, context: &Context) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .filter(|scheme| {
            scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        });
    match scheme {
        Some(scheme) => {
            let scheme = scheme.to_ascii_lowercase();
            let allowed = SAFE_SCHEMES.contains(&scheme.as_str())
                || context
                    .custom_url_schemes
                    .iter()
                    .any(|custom| custom.eq_ignore_ascii_case(&scheme));
            allowed.then(|| url.to_owned())
        }
        None if url.starts_with('/') || url.starts_with('#') => Some(url.to_owned()),
        None => Some(format!("http://{url}")),
    }
}
