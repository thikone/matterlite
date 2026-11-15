// SPDX-License-Identifier: AGPL-3.0-only

//! Mattermost themes (MatterLite.md §11.3, §11.4): the webapp's five built-in themes and the
//! conversion of a Mattermost theme (the JSON in the `theme` preference) into a [`WarpTheme`]
//! with its full chat palette.
//!
//! Background, foreground and accent come from `centerChannelBg`, `centerChannelColor` and
//! `buttonBg`; `details` from the background's luminance; the ANSI colours (used for code
//! highlighting) from `codeTheme`; every other key goes into the chat palette.

use anyhow::{Context as _, Result, anyhow};
use pathfinder_color::ColorU;
use warp_core::ui::color::contrast::relative_luminance;
use warp_core::ui::theme::chat::{ChatColorOverrides, parse_css_color};
use warp_core::ui::theme::{AnsiColor, AnsiColors, Details, Fill, TerminalColors, WarpTheme};

use crate::default_themes::{
    dark_mode_colors, light_mode_colors, solarized_dark_colors, solarized_light_colors,
};

/// The webapp's built-in themes, as in
/// `webapp/channels/src/packages/mattermost-redux/src/constants/preferences.ts` (`THEMES`).
pub const DENIM: &str = r##"{"type":"Denim","sidebarBg":"#1e325c","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#28427b","sidebarTextActiveBorder":"#5d89ea","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#192a4d","sidebarHeaderTextColor":"#ffffff","sidebarTeamBarBg":"#162545","onlineIndicator":"#3db887","awayIndicator":"#ffbc1f","dndIndicator":"#d24b4e","mentionBg":"#ffffff","mentionColor":"#1e325c","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#cc8f00","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#ffd470","mentionHighlightLink":"#1b1d22","codeTheme":"github"}"##;
pub const SAPPHIRE: &str = r##"{"type":"Sapphire","sidebarBg":"#1543a3","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#2a58ba","sidebarTextActiveBorder":"#57b5f0","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#123b91","sidebarHeaderTextColor":"#ffffff","sidebarTeamBarBg":"#123582","onlineIndicator":"#3db887","awayIndicator":"#ffbc1f","dndIndicator":"#d24b4e","mentionBg":"#ffffff","mentionColor":"#174ab5","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#15b7b7","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#7ff0f0","mentionHighlightLink":"#0d6e6e","codeTheme":"github"}"##;
pub const QUARTZ: &str = r##"{"type":"Quartz","sidebarBg":"#f4f4f6","sidebarText":"#1f2228","sidebarUnreadText":"#1f2228","sidebarTextHoverBg":"#e8eaed","sidebarTextActiveBorder":"#32a4ec","sidebarTextActiveColor":"#1f2228","sidebarHeaderBg":"#ebecef","sidebarHeaderTextColor":"#1f2228","sidebarTeamBarBg":"#e5e7eb","onlineIndicator":"#3db887","awayIndicator":"#f5ab07","dndIndicator":"#d24b4e","mentionBg":"#1c58d9","mentionColor":"#ffffff","centerChannelBg":"#ffffff","centerChannelColor":"#3f4350","newMessageSeparator":"#15b7b7","linkColor":"#1c58d9","buttonBg":"#1c58d9","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#7ff0f0","mentionHighlightLink":"#0d6e6e","codeTheme":"github"}"##;
pub const INDIGO: &str = r##"{"type":"Indigo","sidebarBg":"#151e32","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#222c3f","sidebarTextActiveBorder":"#4a7ce8","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#182339","sidebarHeaderTextColor":"#ffffff","sidebarTeamBarBg":"#1c2740","onlineIndicator":"#3db887","awayIndicator":"#f5ab00","dndIndicator":"#d24b4e","mentionBg":"#4a7ce8","mentionColor":"#ffffff","centerChannelBg":"#111827","centerChannelColor":"#dddfe4","newMessageSeparator":"#81a3ef","linkColor":"#5d89ea","buttonBg":"#4a7ce8","buttonColor":"#ffffff","errorTextColor":"#d24b4e","mentionHighlightBg":"#133a91","mentionHighlightLink":"#a4f4f4","codeTheme":"solarized-dark"}"##;
pub const ONYX: &str = r##"{"type":"Onyx","sidebarBg":"#202228","sidebarText":"#ffffff","sidebarUnreadText":"#ffffff","sidebarTextHoverBg":"#25262a","sidebarTextActiveBorder":"#4a7ce8","sidebarTextActiveColor":"#ffffff","sidebarHeaderBg":"#24272d","sidebarHeaderTextColor":"#ffffff","sidebarTeamBarBg":"#292c33","onlineIndicator":"#3db887","awayIndicator":"#f5ab00","dndIndicator":"#d24b4e","mentionBg":"#4b7ce7","mentionColor":"#ffffff","centerChannelBg":"#191b1f","centerChannelColor":"#e3e4e8","newMessageSeparator":"#1adbdb","linkColor":"#5d89ea","buttonBg":"#4a7ce8","buttonColor":"#ffffff","errorTextColor":"#da6c6e","mentionHighlightBg":"#0d6e6e","mentionHighlightLink":"#a4f4f4","codeTheme":"monokai"}"##;

/// The webapp's default theme when the user has none and the server sets no `DefaultTheme`.
pub const DEFAULT_KEY: &str = "denim";

/// A built-in theme by its key (`denim`, `sapphire`, `quartz`, `indigo`, `onyx`), as the
/// server's `DefaultTheme` setting names them.
pub fn builtin_json(key: &str) -> Option<&'static str> {
    Some(match key {
        "denim" => DENIM,
        "sapphire" => SAPPHIRE,
        "quartz" => QUARTZ,
        "indigo" => INDIGO,
        "onyx" => ONYX,
        _ => return None,
    })
}

/// A built-in theme, named "Mattermost <Type>".
pub fn builtin(key: &str) -> WarpTheme {
    let json = builtin_json(key).unwrap_or(DENIM);
    let mut theme = from_mattermost_json(json).expect("built-in Mattermost themes parse");
    let name = theme.name().unwrap_or_default();
    theme.set_name(format!("Mattermost {name}"));
    theme
}

/// Converts a Mattermost theme (the `theme` preference's JSON) into a theme named after its
/// `type` ("Denim", "custom", …). Missing chat keys are derived as for Warp themes.
pub fn from_mattermost_json(json: &str) -> Result<WarpTheme> {
    let value: serde_json::Value = serde_json::from_str(json).context("theme JSON")?;
    let map = value
        .as_object()
        .ok_or_else(|| anyhow!("the theme JSON isn't an object"))?;
    let get = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
    let color = |key: &str| get(key).and_then(parse_css_color);

    let background = color("centerChannelBg").unwrap_or(ColorU::white());
    let foreground = color("centerChannelColor").unwrap_or(ColorU::new(0x3f, 0x43, 0x50, 0xff));
    let accent = color("buttonBg").unwrap_or(ColorU::new(0x1c, 0x58, 0xd9, 0xff));
    let dark = relative_luminance(background) < 0.5;
    let terminal_colors = match get("codeTheme") {
        Some("github") => github_light_colors(),
        Some("solarized-dark") => solarized_dark_colors(),
        Some("solarized-light") => solarized_light_colors(),
        Some("monokai") => monokai_colors(),
        _ if dark => dark_mode_colors(),
        _ => light_mode_colors(),
    };
    let name = get("type")
        .filter(|name| !name.is_empty())
        .unwrap_or("custom");
    let theme = WarpTheme::new(
        Fill::Solid(background),
        foreground,
        Fill::Solid(accent),
        None,
        Some(if dark {
            Details::Darker
        } else {
            Details::Lighter
        }),
        terminal_colors,
        None,
        Some(name.to_owned()),
    );
    Ok(theme.with_chat(ChatColorOverrides::from_mattermost(get)))
}

// GitHub (light), the webapp's `github` code theme, as terminal colours: Primer's palette.
const GITHUB_LIGHT_NORMAL_COLORS: AnsiColors = AnsiColors::new(
    AnsiColor::from_u32(0x24292EFF),
    AnsiColor::from_u32(0xD73A49FF),
    AnsiColor::from_u32(0x22863AFF),
    AnsiColor::from_u32(0xB08800FF),
    AnsiColor::from_u32(0x0366D6FF),
    AnsiColor::from_u32(0x6F42C1FF),
    AnsiColor::from_u32(0x1B7C83FF),
    AnsiColor::from_u32(0x6A737DFF),
);
const GITHUB_LIGHT_BRIGHT_COLORS: AnsiColors = AnsiColors::new(
    AnsiColor::from_u32(0x959DA5FF),
    AnsiColor::from_u32(0xCB2431FF),
    AnsiColor::from_u32(0x28A745FF),
    AnsiColor::from_u32(0xDBAB09FF),
    AnsiColor::from_u32(0x2188FFFF),
    AnsiColor::from_u32(0x8A63D2FF),
    AnsiColor::from_u32(0x3192AAFF),
    AnsiColor::from_u32(0xD1D5DAFF),
);

// Monokai, the webapp's `monokai` code theme.
const MONOKAI_NORMAL_COLORS: AnsiColors = AnsiColors::new(
    AnsiColor::from_u32(0x272822FF),
    AnsiColor::from_u32(0xF92672FF),
    AnsiColor::from_u32(0xA6E22EFF),
    AnsiColor::from_u32(0xF4BF75FF),
    AnsiColor::from_u32(0x66D9EFFF),
    AnsiColor::from_u32(0xAE81FFFF),
    AnsiColor::from_u32(0xA1EFE4FF),
    AnsiColor::from_u32(0xF8F8F2FF),
);
const MONOKAI_BRIGHT_COLORS: AnsiColors = AnsiColors::new(
    AnsiColor::from_u32(0x75715EFF),
    AnsiColor::from_u32(0xFD5FF0FF),
    AnsiColor::from_u32(0xB6F246FF),
    AnsiColor::from_u32(0xFFD787FF),
    AnsiColor::from_u32(0x8AE5F7FF),
    AnsiColor::from_u32(0xC6A2FFFF),
    AnsiColor::from_u32(0xBDF6EFFF),
    AnsiColor::from_u32(0xF9F8F5FF),
);

pub fn github_light_colors() -> TerminalColors {
    TerminalColors::new(GITHUB_LIGHT_NORMAL_COLORS, GITHUB_LIGHT_BRIGHT_COLORS)
}

pub fn monokai_colors() -> TerminalColors {
    TerminalColors::new(MONOKAI_NORMAL_COLORS, MONOKAI_BRIGHT_COLORS)
}

#[cfg(test)]
#[path = "mattermost_tests.rs"]
mod tests;
