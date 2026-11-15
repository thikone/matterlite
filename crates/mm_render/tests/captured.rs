// SPDX-License-Identifier: AGPL-3.0-only

//! Renders the posts captured from the test server's `#dev`, the rendering showcase
//! (`docs/MatterLite-TestServer.md`), with the captured users and custom emoji.

use mm_model::{ChannelId, Emoji, PostList, User, UserId};
use mm_render::{
    Alignment, Block, ChannelInfo, Context, EmojiRef, Highlight, Lookup, SpanKind, UserInfo,
    render_post,
};

struct Captured {
    users: Vec<User>,
    emoji: Vec<Emoji>,
}

impl Lookup for Captured {
    fn user(&self, username: &str) -> Option<UserInfo> {
        let user = self.users.iter().find(|user| user.username == username)?;
        Some(UserInfo {
            id: user.id.clone(),
            display_name: user.username.clone(),
        })
    }

    fn channel(&self, name: &str) -> Option<ChannelInfo> {
        (name == "dev").then(|| ChannelInfo {
            id: ChannelId::new("dev"),
            display_name: "Dev".to_owned(),
        })
    }

    fn custom_emoji(&self, name: &str) -> bool {
        self.emoji.iter().any(|emoji| emoji.name == name)
    }
}

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mm_model/fixtures/captured")
        .join(name);
    std::fs::read_to_string(path).expect("fixture")
}

#[test]
fn the_dev_showcase_renders() {
    let mut users: Vec<User> = serde_json::from_str(&fixture("users.json")).unwrap();
    // `alice` herself and the system users the posts name.
    for name in ["alice", "sysadmin", "testbot"] {
        users.push(User {
            id: UserId::new(format!("{name}-id")),
            username: name.to_owned(),
            ..User::default()
        });
    }
    let lookup = Captured {
        users,
        emoji: serde_json::from_str(&fixture("emoji.json")).unwrap(),
    };
    let context = Context::new(&lookup, "alice");
    let posts: PostList = serde_json::from_str(&fixture("dev_posts.json")).unwrap();
    let rendered: Vec<_> = posts
        .order
        .iter()
        .map(|id| {
            let post = &posts.posts[id];
            (post, render_post(post, &context))
        })
        .collect();

    // System messages read like the webapp's, names as mentions.
    let (_, added) = rendered
        .iter()
        .find(|(post, _)| post.message.starts_with("testbot added"))
        .unwrap();
    assert_eq!(
        added.plain_text(),
        "@testbot added to the channel by @sysadmin."
    );

    // The Markdown showcase.
    let (_, doc) = rendered
        .iter()
        .find(|(post, _)| post.message.starts_with("# Heading"))
        .unwrap();
    let levels: Vec<u8> = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading { level, .. } => Some(*level),
            _ => None,
        })
        .collect();
    assert_eq!(levels, [1, 2, 3]);
    let spans: Vec<_> = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph(spans) => Some(spans),
            _ => None,
        })
        .flatten()
        .collect();
    let has = |test: &dyn Fn(&SpanKind, &str) -> bool| {
        spans.iter().any(|span| test(&span.kind, &span.text))
    };
    assert!(has(
        &|kind, text| matches!(kind, SpanKind::Link(url) if url == "https://mattermost.com")
            && text == "link"
    ));
    assert!(has(&|kind, _| matches!(kind, SpanKind::Link(url)
        if url == "https://github.com/mattermost/mattermost")));
    for name in ["smile", "+1", "tada"] {
        assert!(
            has(
                &|kind, _| matches!(kind, SpanKind::Emoji(EmojiRef::System { name: found, .. })
                if found == name)
            ),
            "{name}"
        );
    }
    assert!(has(
        &|kind, _| matches!(kind, SpanKind::Emoji(EmojiRef::Custom { name })
        if name == "matterlite")
    ));
    assert!(has(&|kind, text| matches!(kind, SpanKind::Channel(channel)
        if channel.display_name == "Dev")
        && text == "~dev"));
    assert!(has(
        &|kind, text| *kind == SpanKind::Hashtag && text == "#hashtag"
    ));
    assert!(
        spans
            .iter()
            .any(|span| span.style.bold && span.style.italic)
    );
    assert!(has(&|kind, _| *kind == SpanKind::LineBreak));

    let lists = doc
        .blocks
        .iter()
        .filter(|block| matches!(block, Block::List { .. }))
        .count();
    assert_eq!(lists, 2, "ordered (with a nested list) and tasks");
    let table = doc
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        table.alignments,
        [Alignment::Left, Alignment::Center, Alignment::Right]
    );
    assert_eq!(table.rows.len(), 2);
    let code: Vec<_> = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Code(code) => Some(code),
            _ => None,
        })
        .collect();
    assert_eq!(code[0].language.as_deref(), Some("rust"));
    assert!(
        code[0]
            .lines
            .iter()
            .flatten()
            .any(|span| span.highlight == Some(Highlight::String))
    );
    assert_eq!(code[1].language.as_deref(), Some("latex"));
    assert!(doc.blocks.contains(&Block::Rule));

    // `@here` and a mention of me.
    let here = rendered
        .iter()
        .find(|(post, _)| post.message.starts_with("@here"))
        .map(|(_, doc)| doc)
        .unwrap();
    let Block::Paragraph(spans) = &here.blocks[0] else {
        panic!()
    };
    assert!(matches!(&spans[0].kind, SpanKind::Mention(mention) if mention.special));
    let review = rendered
        .iter()
        .find(|(post, _)| post.message.starts_with("@alice please"))
        .map(|(_, doc)| doc)
        .unwrap();
    let Block::Paragraph(spans) = &review.blocks[0] else {
        panic!()
    };
    assert!(spans[0].style.highlight, "a mention of me is highlighted");
}
