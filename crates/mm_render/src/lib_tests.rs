// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

struct Known;

impl Lookup for Known {
    fn user(&self, username: &str) -> Option<UserInfo> {
        ["alice", "bob", "dot.name"]
            .contains(&username)
            .then(|| UserInfo {
                id: UserId::new(format!("{username}-id")),
                display_name: format!("{username} display"),
            })
    }

    fn channel(&self, name: &str) -> Option<ChannelInfo> {
        (name == "town-square").then(|| ChannelInfo {
            id: ChannelId::new("town-id"),
            display_name: "Town Square".to_owned(),
        })
    }

    fn custom_emoji(&self, name: &str) -> bool {
        name == "partyparrot"
    }
}

fn context() -> Context<'static> {
    let mut context = Context::new(&Known, "Alice");
    context.mention_keys = vec!["alice".to_owned(), "deploy team".to_owned()];
    context
}

fn spans(message: &str) -> Vec<Span> {
    match render(message, &context()).blocks.as_slice() {
        [Block::Paragraph(spans)] => spans.clone(),
        other => panic!("not one paragraph: {other:?}"),
    }
}

/// The spans as `kind:text` strings, plain text as itself.
fn tokens(message: &str) -> Vec<String> {
    spans(message)
        .iter()
        .map(|span| match &span.kind {
            SpanKind::Text if span.style.highlight => format!("hl:{}", span.text),
            SpanKind::Text => span.text.clone(),
            SpanKind::Link(url) => format!("link({url}):{}", span.text),
            SpanKind::Mention(mention) if mention.me => format!("me:{}", span.text),
            SpanKind::Mention(_) => format!("mention:{}", span.text),
            SpanKind::Channel(channel) => {
                format!("channel({}):{}", channel.display_name, span.text)
            }
            SpanKind::Emoji(emoji) => format!("emoji({}):{}", emoji.name(), span.text),
            SpanKind::Hashtag => format!("tag:{}", span.text),
            SpanKind::Image { url, width, height } => {
                format!("img({url} {width:?} {height:?}):{}", span.text)
            }
            SpanKind::LineBreak => "\\n".to_owned(),
        })
        .collect()
}

#[test]
fn markdown_blocks() {
    let doc = render(
        "# Title\n\n> quoted\n\n- [x] done\n- [ ] todo\n\n3. three\n4. four\n\n---\n\n| a | b |\n|:-|-:|\n| 1 | 2 |",
        &context(),
    );
    let [
        Block::Heading { level: 1, spans },
        Block::Quote(quote),
        Block::List {
            start: None,
            items: tasks,
        },
        Block::List {
            start: Some(3),
            items: numbered,
        },
        Block::Rule,
        Block::Table(table),
    ] = doc.blocks.as_slice()
    else {
        panic!("{:?}", doc.blocks);
    };
    assert_eq!(spans[0].text, "Title");
    assert!(matches!(quote.as_slice(), [Block::Paragraph(_)]));
    assert_eq!(
        tasks.iter().map(|item| item.task).collect::<Vec<_>>(),
        [Some(true), Some(false)]
    );
    assert_eq!(numbered.len(), 2);
    assert_eq!(table.alignments, [Alignment::Left, Alignment::Right]);
    assert_eq!(table.header[1][0].text, "b");
    assert_eq!(table.rows[0][0][0].text, "1");
}

#[test]
fn inline_styles_breaks_and_raw_html() {
    let spans = spans("**bold** _it_ ~~gone~~ `code`\nnext <b>raw</b>");
    assert!(spans[0].style.bold && spans[0].text == "bold");
    assert!(
        spans
            .iter()
            .any(|span| span.style.italic && span.text == "it")
    );
    assert!(
        spans
            .iter()
            .any(|span| span.style.strike && span.text == "gone")
    );
    assert!(
        spans
            .iter()
            .any(|span| span.style.code && span.text == "code")
    );
    // A single newline is a line break, as in Mattermost.
    assert!(spans.iter().any(|span| span.kind == SpanKind::LineBreak));
    // Raw HTML is shown, not interpreted.
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    assert!(text.ends_with("next <b>raw</b>"), "{text}");
}

#[test]
fn mentions() {
    assert_eq!(
        tokens("hi @bob and @alice, @Unknown @channel"),
        [
            "hi ",
            "mention:@bob",
            " and ",
            "me:@alice",
            ", @Unknown ",
            "me:@channel"
        ]
    );
    // Trailing punctuation isn't part of a name, but dots inside are.
    assert_eq!(tokens("ask @bob."), ["ask ", "mention:@bob", "."]);
    assert_eq!(tokens("@dot.name!"), ["mention:@dot.name", "!"]);
    // Not in the middle of a word (e-mail addresses become links).
    assert_eq!(
        tokens("mail bob@example.com"),
        ["mail ", "link(mailto:bob@example.com):bob@example.com"]
    );
    let SpanKind::Mention(mention) = &spans("@bob")[0].kind else {
        panic!()
    };
    assert_eq!(mention.user.as_ref().map(UserId::as_str), Some("bob-id"));
    assert_eq!(mention.display_name.as_deref(), Some("bob display"));
}

#[test]
fn my_mention_keys_are_highlighted_as_whole_words() {
    assert_eq!(
        tokens("Alice and the Deploy Team, not alicex"),
        ["hl:Alice", " and the ", "hl:Deploy Team", ", not alicex"]
    );
}

#[test]
fn channels_from_props_then_known_channels() {
    let post: Post = serde_json::from_value(json!({
        "message": "see ~dev and ~town-square and ~nowhere",
        "props": {"channel_mentions": {"dev": {"display_name": "Dev", "team_name": "other"}}}
    }))
    .unwrap();
    let doc = render_post(&post, &context());
    let [Block::Paragraph(spans)] = doc.blocks.as_slice() else {
        panic!()
    };
    let channels: Vec<_> = spans
        .iter()
        .filter_map(|span| match &span.kind {
            SpanKind::Channel(channel) => Some((
                channel.display_name.as_str(),
                channel.team.as_deref(),
                span.text.as_str(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        channels,
        [
            ("Dev", Some("other"), "~dev"),
            ("Town Square", None, "~town-square")
        ]
    );
}

#[test]
fn hashtags() {
    assert_eq!(
        tokens("#release and #ab but #v1.2. issue#123"),
        ["tag:#release", " and #ab but ", "tag:#v1.2", ". issue#123"]
    );
}

#[test]
fn emoji_shortcodes_emoticons_and_unicode() {
    assert_eq!(
        tokens(":+1: :partyparrot: :nope: :) 👋🏻"),
        [
            "emoji(+1)::+1:",
            " ",
            "emoji(partyparrot)::partyparrot:",
            " :nope: ",
            "emoji(slightly_smiling_face)::)",
            " ",
            "emoji(wave_light_skin_tone):👋🏻"
        ]
    );
    // No emoticons inside words.
    assert_eq!(tokens("a:)b"), ["a:)b"]);
    let SpanKind::Emoji(EmojiRef::System { image, .. }) = &spans(":smile:")[0].kind else {
        panic!()
    };
    assert_eq!(image, "emoji/1f604.png");
}

#[test]
fn jumbo_emoji() {
    assert!(render(":tada: :+1:", &context()).jumbo_emoji);
    assert!(render("🎉", &context()).jumbo_emoji);
    assert!(!render("yay :tada:", &context()).jumbo_emoji);
    assert!(!render(&":tada: ".repeat(9), &context()).jumbo_emoji);
}

#[test]
fn links_and_images() {
    assert_eq!(
        tokens("[site](example.com) [bad](javascript:alert(1)) https://x.org/a"),
        [
            "link(http://example.com):site",
            " bad ",
            "link(https://x.org/a):https://x.org/a"
        ]
    );
    assert_eq!(
        tokens("![logo](https://x.org/l.png =32x16)"),
        ["img(https://x.org/l.png Some(32) Some(16)):logo"]
    );
    assert_eq!(
        tokens("![w](https://x.org/l.png =40)"),
        ["img(https://x.org/l.png Some(40) None):w"]
    );
    // Formatting doesn't happen inside links.
    assert_eq!(
        tokens("[@bob](https://x.org)"),
        ["link(https://x.org):@bob"]
    );
    let mut custom = context();
    custom.custom_url_schemes = vec!["jira".to_owned()];
    let doc = render("[t](jira:ABC-1)", &custom);
    let [Block::Paragraph(spans)] = doc.blocks.as_slice() else {
        panic!()
    };
    assert_eq!(spans[0].kind, SpanKind::Link("jira:ABC-1".to_owned()));
}

#[test]
fn fenced_code_is_highlighted() {
    let doc = render(
        "```rust\nfn main() {\n    let x = \"hi\"; // greet\n}\n```",
        &context(),
    );
    let [Block::Code(code)] = doc.blocks.as_slice() else {
        panic!()
    };
    assert_eq!(code.language.as_deref(), Some("rust"));
    assert_eq!(code.lines.len(), 3);
    let class = |text: &str| {
        code.lines
            .iter()
            .flatten()
            .find(|span| span.text.contains(text))
            .and_then(|span| span.highlight)
    };
    assert_eq!(class("fn"), Some(Highlight::Keyword));
    assert_eq!(class("\"hi\""), Some(Highlight::String));
    assert_eq!(class("// greet"), Some(Highlight::Comment));
    // Unknown languages and LaTeX stay plain, with their lines.
    let doc = render("```latex\nx^2\n```\n```nope\na\nb\n```", &context());
    for block in &doc.blocks {
        let Block::Code(code) = block else { panic!() };
        assert!(
            code.lines
                .iter()
                .flatten()
                .all(|span| span.highlight.is_none())
        );
    }
}

#[test]
fn system_messages_and_me_posts() {
    let post = |kind: &str, props: serde_json::Value| -> Post {
        serde_json::from_value(json!({
            "type": kind, "message": "server text", "props": props
        }))
        .unwrap()
    };
    let text = |post: Post| render_post(&post, &context()).plain_text();
    assert_eq!(
        text(post("system_join_channel", json!({"username": "bob"}))),
        "@bob joined the channel."
    );
    assert_eq!(
        text(post(
            "system_add_to_channel",
            json!({"username": "alice", "addedUsername": "bob"})
        )),
        "@bob added to the channel by @alice."
    );
    assert_eq!(
        text(post(
            "system_purpose_change",
            json!({"username": "bob", "new_purpose": "Ship it"})
        )),
        "@bob updated the channel purpose to: Ship it"
    );
    assert_eq!(
        text(post(
            "system_header_change",
            json!({"username": "bob", "old_header": "a", "new_header": "b"})
        )),
        "@bob updated the channel header From: a To: b"
    );
    // Unknown types and missing props: the server's text.
    assert_eq!(text(post("system_generic", json!({}))), "server text");
    assert_eq!(text(post("system_join_channel", json!({}))), "server text");
    let me = render_post(&post("me", json!({})), &context());
    let [Block::Paragraph(spans)] = me.blocks.as_slice() else {
        panic!()
    };
    assert!(spans.iter().all(|span| span.style.italic));
}

#[test]
fn plain_text_for_excerpts() {
    let doc = render("**Deploy** done :tada:\n\n```\nlog\n```", &context());
    assert_eq!(doc.plain_text(), "Deploy done 🎉 log");
}
