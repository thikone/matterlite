// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn names_and_aliases() {
    assert_eq!(all().len(), 3300);
    let thumbs = by_name("+1").unwrap();
    assert_eq!(by_name("thumbsup"), Some(thumbs));
    assert_eq!(thumbs.text, "\u{1F44D}");
    assert_eq!(thumbs.image(), "emoji/1f44d.png");
    let wave = by_name("wave_light_skin_tone").unwrap();
    assert_eq!(wave.skin_tone.as_deref(), Some("1F3FB"));
    assert!(by_name("not_an_emoji").is_none());
}

#[test]
fn every_emoji_has_its_image() {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    for emoji in all() {
        assert!(assets.join(emoji.image()).is_file(), "{}", emoji.name());
    }
}

#[test]
fn unicode_sequences_at_the_start() {
    let (emoji, len) = at_start("👋🏻 hi").unwrap();
    assert_eq!(emoji.name(), "wave_light_skin_tone");
    assert_eq!(&"👋🏻 hi"[len..], " hi");
    // A family (ZWJ sequence) is one emoji.
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    assert_eq!(at_start(family).unwrap().1, family.len());
    // Text-default characters need the emoji variation selector.
    assert!(at_start("© 2026").is_none());
    assert!(at_start("❤").is_none());
    let (heart, len) = at_start("❤\u{FE0F}!").unwrap();
    assert_eq!(heart.name(), "heart");
    assert_eq!(len, "❤\u{FE0F}".len());
    // Keycaps, but not plain digits.
    assert_eq!(at_start("1\u{FE0F}\u{20E3}").unwrap().0.name(), "one");
    assert!(at_start("1 apple").is_none());
    assert!(at_start("hello").is_none());
}

#[test]
fn emoticons() {
    assert_eq!(
        emoticon_at_start(":) ok"),
        Some(("slightly_smiling_face", 2))
    );
    assert_eq!(emoticon_at_start(":-)"), Some(("slightly_smiling_face", 3)));
    assert_eq!(emoticon_at_start(":D"), Some(("smile", 2)));
    assert_eq!(emoticon_at_start("</3"), Some(("broken_heart", 3)));
    // Not when a letter follows (`:pizza`, `:octopus`).
    assert_eq!(emoticon_at_start(":pizza"), None);
    assert_eq!(emoticon_at_start(":o."), Some(("open_mouth", 2)));
    for (_, name) in EMOTICONS {
        assert!(by_name(name).is_some(), "{name}");
    }
}
