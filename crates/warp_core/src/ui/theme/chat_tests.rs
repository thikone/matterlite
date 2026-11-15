// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ui::theme::{Details, Fill, mock_terminal_colors};

fn theme(background: u32, foreground: u32, accent: u32, details: Details) -> WarpTheme {
    WarpTheme::new(
        Fill::Solid(ColorU::from_u32(background)),
        ColorU::from_u32(foreground),
        Fill::Solid(ColorU::from_u32(accent)),
        None,
        Some(details),
        mock_terminal_colors(),
        None,
        Some("Test".to_owned()),
    )
}

#[test]
fn derived_palette_follows_the_theme() {
    let dark = theme(0x000000FF, 0xFFFFFFFF, 0x19AAD8FF, Details::Darker);
    let chat = dark.chat();
    let ansi = &dark.terminal_colors().normal;
    assert_eq!(chat.sidebar_text, ColorU::white());
    assert_eq!(chat.sidebar_active_border, ColorU::from_u32(0x19AAD8FF));
    assert_eq!(chat.link, chat.button_bg);
    assert_eq!(chat.online, ansi.green.into());
    assert_eq!(chat.dnd, ansi.red.into());
    assert_eq!(chat.mention_highlight_bg.a, 63);
    // The sidebar stands out from the background, and hovering it shows.
    assert_ne!(chat.sidebar_bg, ColorU::black());
    assert_ne!(chat.sidebar_hover_bg, chat.sidebar_bg);
    // Text on the accent is readable.
    assert_eq!(chat.button_text, ColorU::black());
}

#[test]
fn chat_section_overrides_some_colors() {
    let yaml = r##"
name: Partly chat
accent: '#1c58d9'
background: '#ffffff'
foreground: '#3f4350'
details: lighter
terminal_colors:
  normal: { black: '#000000', red: '#ff0000', green: '#00ff00', yellow: '#ffff00',
            blue: '#0000ff', magenta: '#ff00ff', cyan: '#00ffff', white: '#ffffff' }
  bright: { black: '#000000', red: '#ff0000', green: '#00ff00', yellow: '#ffff00',
            blue: '#0000ff', magenta: '#ff00ff', cyan: '#00ffff', white: '#ffffff' }
chat:
  sidebar_bg: '#1e325c'
  sidebar_text: '#fff'
"##;
    let theme: WarpTheme = serde_yaml::from_str(yaml).unwrap();
    let chat = theme.chat();
    assert_eq!(chat.sidebar_bg, ColorU::from_u32(0x1E325CFF));
    assert_eq!(chat.sidebar_text, ColorU::white());
    // Not in the section: derived.
    assert_eq!(chat.online, ColorU::from_u32(0x00FF00FF));

    // A Warp theme without the section still loads, and writing keeps the section.
    let written = serde_yaml::to_string(&theme).unwrap();
    assert!(written.contains("sidebar_bg: \"#1e325c\""), "{written}");
    let back: WarpTheme = serde_yaml::from_str(&written).unwrap();
    assert_eq!(back, theme);
}

#[test]
fn mattermost_keys_and_css_colors() {
    let overrides = ChatColorOverrides::from_mattermost(|key| match key {
        "sidebarBg" => Some("#1e325c"),
        "mentionHighlightBg" => Some("rgba(255, 212, 112, 0.5)"),
        "onlineIndicator" => Some("not a colour"),
        _ => None,
    });
    assert_eq!(overrides.sidebar_bg, Some(ColorU::from_u32(0x1E325CFF)));
    assert_eq!(
        overrides.mention_highlight_bg,
        Some(ColorU::new(255, 212, 112, 128))
    );
    assert_eq!(overrides.online, None);
    assert_eq!(MATTERMOST_KEYS.len(), 21);
    assert_eq!(
        parse_css_color("rgb(1,2,3)"),
        Some(ColorU::new(1, 2, 3, 255))
    );
    assert_eq!(parse_css_color("#abc"), Some(ColorU::from_u32(0xAABBCCFF)));
    assert_eq!(parse_css_color("blue"), None);
}
