// SPDX-License-Identifier: AGPL-3.0-only

//! Message rendering (MatterLite.md §12): a post's Markdown and Mattermost syntax into a
//! UI-independent [`MessageDoc`] of blocks and styled spans. The UI turns it into elements.
//!
//! ```text
//! message ─ comrak (GFM, no raw HTML) ─► blocks
//!   text outside code and links ─► @mentions, ~channels, #hashtags, :emoji:, emoticons,
//!                                  Unicode emoji, my mention keys highlighted
//!   fenced code ─► tree-sitter highlighting (arborium grammars)
//! ```
//!
//! What the renderer needs to know about users, channels and custom emoji comes through
//! [`Lookup`]. No UI dependency.

pub mod doc;
mod highlight;
mod markdown;
mod system;
mod text;

use mm_model::{ChannelId, Post, UserId};
use serde_json::{Map, Value};

pub use doc::{
    Alignment, Block, ChannelMention, CodeBlock, CodeSpan, EmojiRef, Highlight, ListItem, Mention,
    MessageDoc, Span, SpanKind, Style, Table,
};

/// `@channel`, `@all` and `@here`.
pub const SPECIAL_MENTIONS: [&str; 3] = ["channel", "all", "here"];

/// Messages of up to this many emoji (and nothing else) are drawn large.
pub const MAX_JUMBO_EMOJI: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    pub id: UserId,
    /// In the viewer's name format.
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub display_name: String,
}

/// What the renderer looks up.
pub trait Lookup {
    /// A user by username (lower case).
    fn user(&self, username: &str) -> Option<UserInfo>;
    /// A channel of the current team by name, for `~name` not in the post's
    /// `channel_mentions`.
    fn channel(&self, name: &str) -> Option<ChannelInfo>;
    /// Whether the server has a custom emoji of this name.
    fn custom_emoji(&self, name: &str) -> bool;
}

/// Knows nothing: mentions of unknown users stay text.
pub struct NoLookup;

impl Lookup for NoLookup {
    fn user(&self, _: &str) -> Option<UserInfo> {
        None
    }

    fn channel(&self, _: &str) -> Option<ChannelInfo> {
        None
    }

    fn custom_emoji(&self, _: &str) -> bool {
        false
    }
}

/// Everything rendering a message depends on besides its text.
#[derive(Clone)]
pub struct Context<'a> {
    pub lookup: &'a dyn Lookup,
    /// My username, lower case: mentions of it are highlighted.
    pub my_username: String,
    /// Words to highlight as my mention keys (lower case, without `@`), e.g. my first name
    /// and custom keys from my notification settings.
    pub mention_keys: Vec<String>,
    /// `@channel`, `@all` and `@here` notify me (channel-wide mentions aren't off).
    pub channel_mentions_notify: bool,
    /// The post's `props.channel_mentions` (`{name: {display_name, team_name}}`).
    pub channel_mentions: Option<&'a Map<String, Value>>,
    /// The server's `CustomUrlSchemes`.
    pub custom_url_schemes: Vec<String>,
}

impl<'a> Context<'a> {
    pub fn new(lookup: &'a dyn Lookup, my_username: &str) -> Self {
        Self {
            lookup,
            my_username: my_username.to_lowercase(),
            mention_keys: Vec::new(),
            channel_mentions_notify: true,
            channel_mentions: None,
            custom_url_schemes: Vec::new(),
        }
    }
}

/// Renders Markdown with Mattermost's additions.
pub fn render(message: &str, context: &Context) -> MessageDoc {
    let blocks = markdown::render(message, context);
    let jumbo_emoji = is_jumbo(&blocks);
    MessageDoc {
        blocks,
        jumbo_emoji,
    }
}

/// Renders a post: system messages from their type and props, `/me` posts in italics, the
/// post's `channel_mentions` used for `~channel` links.
pub fn render_post(post: &Post, context: &Context) -> MessageDoc {
    let context = Context {
        channel_mentions: post
            .props
            .get("channel_mentions")
            .and_then(Value::as_object),
        ..context.clone()
    };
    let source = system::markdown(post);
    let mut doc = render(source.as_deref().unwrap_or(&post.message), &context);
    if post.post_type == "me" {
        italicize(&mut doc.blocks);
    }
    doc
}

fn italicize(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => {
                for span in spans {
                    span.style.italic = true;
                }
            }
            Block::Quote(blocks) => italicize(blocks),
            Block::List { items, .. } => {
                for item in items {
                    italicize(&mut item.blocks);
                }
            }
            _ => {}
        }
    }
}

/// One paragraph of emoji and spaces only, at most [`MAX_JUMBO_EMOJI`] of them.
fn is_jumbo(blocks: &[Block]) -> bool {
    let [Block::Paragraph(spans)] = blocks else {
        return false;
    };
    let mut count = 0;
    for span in spans {
        match &span.kind {
            SpanKind::Emoji(_) => count += 1,
            SpanKind::Text if span.text.trim().is_empty() => {}
            _ => return false,
        }
    }
    (1..=MAX_JUMBO_EMOJI).contains(&count)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
