// SPDX-License-Identifier: AGPL-3.0-only

use warp_core::ui::theme::ColorScheme;
use warp_core::ui::theme::chat::ChatColors;

use super::*;

#[test]
fn builtins_carry_the_webapp_colors() {
    let denim = builtin("denim");
    assert_eq!(denim.name().as_deref(), Some("Mattermost Denim"));
    assert_eq!(denim.background().into_solid(), ColorU::white());
    assert_eq!(
        denim.foreground().into_solid(),
        ColorU::from_u32(0x3F4350FF)
    );
    assert_eq!(denim.accent().into_solid(), ColorU::from_u32(0x1C58D9FF));
    assert_eq!(denim.inferred_color_scheme(), ColorScheme::DarkOnLight);
    let chat = denim.chat();
    assert_eq!(chat.sidebar_bg, ColorU::from_u32(0x1E325CFF));
    assert_eq!(chat.sidebar_text, ColorU::white());
    assert_eq!(chat.mention_highlight_bg, ColorU::from_u32(0xFFD470FF));
    let onyx = builtin("onyx");
    assert_eq!(onyx.terminal_colors(), &monokai_colors());
    assert_eq!(onyx.inferred_color_scheme(), ColorScheme::LightOnDark);
    assert_eq!(
        builtin("indigo").terminal_colors(),
        &solarized_dark_colors()
    );
    assert_eq!(builtin("quartz").terminal_colors(), &github_light_colors());
    // Unknown keys fall back to the default.
    assert_eq!(builtin("unknown"), builtin("denim"));
}

#[test]
fn all_builtin_chat_keys_are_set() {
    for key in ["denim", "sapphire", "quartz", "indigo", "onyx"] {
        let theme = builtin(key);
        let overrides = theme.chat_overrides().unwrap();
        let derived = ChatColors::derive(&theme);
        // with_overrides on a palette of only one colour changes all of them.
        let marker = ColorU::new(1, 2, 3, 4);
        let mut flat = derived;
        for field in [
            &mut flat.sidebar_bg,
            &mut flat.sidebar_text,
            &mut flat.sidebar_unread_text,
            &mut flat.sidebar_hover_bg,
            &mut flat.sidebar_active_border,
            &mut flat.sidebar_active_text,
            &mut flat.sidebar_header_bg,
            &mut flat.sidebar_header_text,
            &mut flat.team_bar_bg,
            &mut flat.online,
            &mut flat.away,
            &mut flat.dnd,
            &mut flat.mention_badge_bg,
            &mut flat.mention_badge_text,
            &mut flat.mention_highlight_bg,
            &mut flat.mention_highlight_link,
            &mut flat.new_message_separator,
            &mut flat.link,
            &mut flat.button_bg,
            &mut flat.button_text,
            &mut flat.error_text,
        ] {
            *field = marker;
        }
        let resolved = flat.with_overrides(overrides);
        assert_eq!(resolved, theme.chat(), "{key}");
    }
}

#[test]
fn a_custom_server_theme_converts() {
    let json = r##"{"type":"custom","sidebarBg":"#2f3e4e","centerChannelBg":"#1f1f1f",
        "centerChannelColor":"#dddddd","buttonBg":"#e06c75","mentionHighlightBg":"rgba(255,0,0,0.5)",
        "codeTheme":"solarized-light","extra":true}"##;
    let theme = from_mattermost_json(json).unwrap();
    assert_eq!(theme.name().as_deref(), Some("custom"));
    assert_eq!(theme.terminal_colors(), &solarized_light_colors());
    assert_eq!(theme.inferred_color_scheme(), ColorScheme::LightOnDark);
    let chat = theme.chat();
    assert_eq!(chat.sidebar_bg, ColorU::from_u32(0x2F3E4EFF));
    assert_eq!(chat.mention_highlight_bg, ColorU::new(255, 0, 0, 128));
    // Missing keys are derived from the theme: the accent here.
    assert_eq!(chat.link, ColorU::from_u32(0xE06C75FF));

    // Without a code theme the ANSI colours follow the background.
    let light = from_mattermost_json(r##"{"centerChannelBg":"#fafafa"}"##).unwrap();
    assert_eq!(light.terminal_colors(), &light_mode_colors());
    assert!(from_mattermost_json("[]").is_err());
    assert!(from_mattermost_json("not json").is_err());
}
