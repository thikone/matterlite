// SPDX-License-Identifier: AGPL-3.0-only

//! Mattermost's text formatting on plain text (outside code and links), in the webapp's order
//! (`utils/text_formatting.tsx`, WebApp.md §11.2): `@mentions`, `~channels`, `#hashtags`,
//! `:emoji:` and emoticons, Unicode emoji, then my mention keys highlighted.

use mm_emoji::{at_start, by_name, emoticon_at_start};

use crate::doc::{ChannelMention, EmojiRef, Mention, Span, SpanKind, Style};
use crate::{Context, SPECIAL_MENTIONS};

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn username_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// Splits plain text into spans.
pub(crate) fn format(text: &str, style: Style, context: &Context, out: &mut Vec<Span>) {
    let mut plain_start = 0;
    let mut index = 0;
    let mut previous: Option<char> = None;
    while index < text.len() {
        let rest = &text[index..];
        let c = rest.chars().next().expect("not at the end");
        let at_word_start = !previous.is_some_and(is_word);
        let token = if at_word_start {
            match c {
                '@' => mention(rest, context),
                '~' => channel(rest, context),
                '#' => hashtag(rest),
                ':' => shortcode(rest, context),
                _ => None,
            }
            .or_else(|| emoticon(rest))
        } else {
            None
        }
        .or_else(|| unicode_emoji(rest));
        match token {
            Some((kind, len)) => {
                push_plain(&text[plain_start..index], style, context, out);
                out.push(Span {
                    text: rest[..len].to_owned(),
                    style: Style {
                        highlight: style.highlight
                            || matches!(&kind, SpanKind::Mention(mention) if mention.me),
                        ..style
                    },
                    kind,
                });
                index += len;
                plain_start = index;
                previous = rest[..len].chars().last();
            }
            None => {
                index += c.len_utf8();
                previous = Some(c);
            }
        }
    }
    push_plain(&text[plain_start..], style, context, out);
}

/// `@name`: a user (trying without trailing `.`, `-`, `_` as the webapp does), or
/// `@channel`/`@all`/`@here`. Unknown names stay text.
fn mention(text: &str, context: &Context) -> Option<(SpanKind, usize)> {
    let name_len = text[1..]
        .char_indices()
        .take_while(|(_, c)| username_char(*c))
        .last()
        .map(|(index, c)| index + c.len_utf8())?;
    let mut name = &text[1..1 + name_len];
    loop {
        let lower = name.to_ascii_lowercase();
        if SPECIAL_MENTIONS.contains(&lower.as_str()) {
            let me = context.channel_mentions_notify;
            return Some((
                SpanKind::Mention(Mention {
                    username: lower,
                    user: None,
                    display_name: None,
                    special: true,
                    me,
                }),
                1 + name.len(),
            ));
        }
        if let Some(user) = context.lookup.user(&lower) {
            let me = lower == context.my_username;
            return Some((
                SpanKind::Mention(Mention {
                    username: lower,
                    user: Some(user.id),
                    display_name: Some(user.display_name),
                    special: false,
                    me,
                }),
                1 + name.len(),
            ));
        }
        match name.strip_suffix(['.', '-', '_']) {
            Some(shorter) if !shorter.is_empty() => name = shorter,
            _ => return None,
        }
    }
}

/// `~channel-name`: from the post's `channel_mentions` (the server resolved it), else a
/// channel I know.
fn channel(text: &str, context: &Context) -> Option<(SpanKind, usize)> {
    let len = text[1..]
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .last()
        .map(|(index, c)| index + c.len_utf8())?;
    let mut name = &text[1..1 + len];
    loop {
        let lower = name.to_ascii_lowercase();
        if let Some(found) = context.channel_mention(&lower) {
            return Some((SpanKind::Channel(found), 1 + name.len()));
        }
        match name.strip_suffix(['-', '_']) {
            Some(shorter) if !shorter.is_empty() => name = shorter,
            _ => return None,
        }
    }
}

/// `#tag`: a letter, then at least two more letters, digits, `-`, `_` or `.`; a trailing `.`
/// isn't part of it.
fn hashtag(text: &str) -> Option<(SpanKind, usize)> {
    let mut chars = text[1..].char_indices();
    let (_, first) = chars.next()?;
    if !first.is_alphabetic() {
        return None;
    }
    let mut len = first.len_utf8();
    for (index, c) in chars {
        if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
            len = index + c.len_utf8();
        } else {
            break;
        }
    }
    let tag = text[1..1 + len].trim_end_matches('.');
    (tag.chars().count() >= 3).then(|| (SpanKind::Hashtag, 1 + tag.len()))
}

/// `:name:` for a system or custom emoji, not followed by a word character.
fn shortcode(text: &str, context: &Context) -> Option<(SpanKind, usize)> {
    let end = text[1..].find(':')? + 1;
    let name = &text[1..end];
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '+' | '-'))
        || text[end + 1..].chars().next().is_some_and(is_word)
    {
        return None;
    }
    let emoji = match by_name(name) {
        Some(emoji) => EmojiRef::System {
            name: emoji.name().to_owned(),
            image: emoji.image(),
        },
        None if context.lookup.custom_emoji(name) => EmojiRef::Custom {
            name: name.to_owned(),
        },
        None => return None,
    };
    Some((SpanKind::Emoji(emoji), end + 1))
}

fn emoticon(text: &str) -> Option<(SpanKind, usize)> {
    let (name, len) = emoticon_at_start(text)?;
    let emoji = by_name(name)?;
    Some((
        SpanKind::Emoji(EmojiRef::System {
            name: emoji.name().to_owned(),
            image: emoji.image(),
        }),
        len,
    ))
}

fn unicode_emoji(text: &str) -> Option<(SpanKind, usize)> {
    let (emoji, len) = at_start(text)?;
    Some((
        SpanKind::Emoji(EmojiRef::System {
            name: emoji.name().to_owned(),
            image: emoji.image(),
        }),
        len,
    ))
}

/// Plain text, with my mention keys highlighted (whole words, ignoring case).
fn push_plain(text: &str, style: Style, context: &Context, out: &mut Vec<Span>) {
    if text.is_empty() {
        return;
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let lower = text.to_lowercase();
    // Lower-casing can change byte lengths; only highlight when it doesn't.
    if lower.len() == text.len() {
        for key in &context.mention_keys {
            if key.is_empty() {
                continue;
            }
            let mut from = 0;
            while let Some(found) = lower[from..].find(key.as_str()) {
                let start = from + found;
                let end = start + key.len();
                let before = text[..start].chars().next_back();
                let after = text[end..].chars().next();
                if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
                    ranges.push((start, end));
                }
                from = end;
            }
        }
    }
    ranges.sort_unstable();
    let mut position = 0;
    for (start, end) in ranges {
        if start < position {
            continue;
        }
        if start > position {
            out.push(Span::text(&text[position..start], style));
        }
        out.push(Span::text(
            &text[start..end],
            Style {
                highlight: true,
                ..style
            },
        ));
        position = end;
    }
    if position < text.len() {
        out.push(Span::text(&text[position..], style));
    }
}

impl Context<'_> {
    fn channel_mention(&self, name: &str) -> Option<ChannelMention> {
        if let Some(found) = self
            .channel_mentions
            .and_then(|mentions| mentions.get(name))
            .and_then(serde_json::Value::as_object)
        {
            let display_name = found
                .get("display_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(name)
                .to_owned();
            let team = found
                .get("team_name")
                .and_then(serde_json::Value::as_str)
                .filter(|team| !team.is_empty())
                .map(str::to_owned);
            let channel = self.lookup.channel(name).map(|known| known.id);
            return Some(ChannelMention {
                name: name.to_owned(),
                display_name,
                channel,
                team,
            });
        }
        let known = self.lookup.channel(name)?;
        Some(ChannelMention {
            name: name.to_owned(),
            display_name: known.display_name,
            channel: Some(known.id),
            team: None,
        })
    }
}
