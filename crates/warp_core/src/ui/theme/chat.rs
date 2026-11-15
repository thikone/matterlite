// SPDX-License-Identifier: AGPL-3.0-only

//! MatterLite's chat palette (MatterLite.md §11.2): the colours a chat UI needs that a terminal
//! theme doesn't have (sidebar, status indicators, mention badges and highlights).
//!
//! A theme can set any of them in an optional `chat` section (YAML keys in snake_case, the same
//! names as Mattermost's theme JSON in camelCase); the others are derived from the theme with
//! [`ChatColors::derive`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use warpui_core::color::ColorU;

use super::WarpTheme;
use crate::ui::color::blend::Blend as _;
use crate::ui::color::coloru_with_opacity;
use crate::ui::color::contrast::{MinimumAllowedContrast, pick_best_foreground_color};
use crate::ui::color::hex_color::coloru_from_hex_string;

macro_rules! chat_colors {
    ($($(#[$doc:meta])* $field:ident => $mattermost:literal,)*) => {
        /// The resolved chat palette of a theme.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct ChatColors {
            $($(#[$doc])* pub $field: ColorU,)*
        }

        /// The `chat` section of a theme: the colours it sets explicitly.
        #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub struct ChatColorOverrides {
            $(
                #[serde(default, with = "optional_hex", skip_serializing_if = "Option::is_none")]
                pub $field: Option<ColorU>,
            )*
        }

        impl ChatColors {
            /// These colours with the ones `overrides` sets replaced.
            pub fn with_overrides(mut self, overrides: &ChatColorOverrides) -> Self {
                $(if let Some(color) = overrides.$field {
                    self.$field = color;
                })*
                self
            }
        }

        impl ChatColorOverrides {
            /// Reads the keys of a Mattermost theme (`sidebarBg`, …); `get` returns the value of
            /// a key. Values that aren't colours are skipped.
            pub fn from_mattermost<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
                Self {
                    $($field: get($mattermost).and_then(parse_css_color),)*
                }
            }

            /// Whether no colour is set.
            pub fn is_empty(&self) -> bool {
                $(self.$field.is_none())&&*
            }
        }

        /// Chat colour fields and their Mattermost theme keys.
        pub const MATTERMOST_KEYS: &[(&str, &str)] = &[$((stringify!($field), $mattermost),)*];
    };
}

chat_colors! {
    /// The Chats tab background.
    sidebar_bg => "sidebarBg",
    /// Channel names in the Chats tab (read ones are drawn at reduced opacity).
    sidebar_text => "sidebarText",
    sidebar_unread_text => "sidebarUnreadText",
    sidebar_hover_bg => "sidebarTextHoverBg",
    sidebar_active_border => "sidebarTextActiveBorder",
    sidebar_active_text => "sidebarTextActiveColor",
    sidebar_header_bg => "sidebarHeaderBg",
    sidebar_header_text => "sidebarHeaderTextColor",
    team_bar_bg => "sidebarTeamBarBg",
    online => "onlineIndicator",
    away => "awayIndicator",
    dnd => "dndIndicator",
    mention_badge_bg => "mentionBg",
    mention_badge_text => "mentionColor",
    mention_highlight_bg => "mentionHighlightBg",
    mention_highlight_link => "mentionHighlightLink",
    new_message_separator => "newMessageSeparator",
    link => "linkColor",
    button_bg => "buttonBg",
    button_text => "buttonColor",
    error_text => "errorTextColor",
}

impl ChatColors {
    /// The palette of a theme without a `chat` section (MatterLite.md §11.2, "Derived palette").
    pub fn derive(theme: &WarpTheme) -> Self {
        let background = theme.background().into_solid();
        let foreground = theme.foreground().into_solid();
        let accent = theme.accent().into_solid();
        let ansi = &theme.terminal_colors().normal;
        let sidebar_bg = theme.surface_1().into_solid();
        let header_bg = theme.surface_2().into_solid();
        let on_accent = pick_best_foreground_color(
            accent,
            background,
            foreground,
            MinimumAllowedContrast::Text,
        );
        let red: ColorU = ansi.red.into();
        Self {
            sidebar_bg,
            sidebar_text: foreground,
            sidebar_unread_text: foreground,
            sidebar_hover_bg: sidebar_bg.blend(&coloru_with_opacity(foreground, 8)),
            sidebar_active_border: accent,
            sidebar_active_text: foreground,
            sidebar_header_bg: header_bg,
            sidebar_header_text: foreground,
            team_bar_bg: header_bg,
            online: ansi.green.into(),
            away: ansi.yellow.into(),
            dnd: red,
            mention_badge_bg: accent,
            mention_badge_text: on_accent,
            mention_highlight_bg: coloru_with_opacity(ansi.yellow.into(), 25),
            mention_highlight_link: foreground,
            new_message_separator: red,
            link: accent,
            button_bg: accent,
            button_text: on_accent,
            error_text: red,
        }
    }
}

impl WarpTheme {
    /// The theme's chat palette: its `chat` section over the derived colours.
    pub fn chat(&self) -> ChatColors {
        let derived = ChatColors::derive(self);
        match &self.chat {
            Some(overrides) => derived.with_overrides(overrides),
            None => derived,
        }
    }

    pub fn chat_overrides(&self) -> Option<&ChatColorOverrides> {
        self.chat.as_ref()
    }

    /// The theme with this `chat` section.
    pub fn with_chat(mut self, overrides: ChatColorOverrides) -> Self {
        self.chat = (!overrides.is_empty()).then_some(overrides);
        self
    }
}

/// A colour as Mattermost themes write it: `#rgb`, `#rrggbb`, `rgb(r,g,b)` or
/// `rgba(r,g,b,a)` with `a` from 0 to 1.
pub fn parse_css_color(value: &str) -> Option<ColorU> {
    let value = value.trim();
    if value.starts_with('#') {
        return coloru_from_hex_string(value).ok();
    }
    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let channel = |index: usize| parts.get(index)?.parse::<u8>().ok();
    let alpha = match parts.get(3) {
        Some(alpha) => (alpha.parse::<f32>().ok()?.clamp(0., 1.) * 255.).round() as u8,
        None => 255,
    };
    if parts.len() > 4 {
        return None;
    }
    Some(ColorU::new(channel(0)?, channel(1)?, channel(2)?, alpha))
}

/// `#rrggbb` (or `#rrggbbaa` when not opaque) for an optional colour.
mod optional_hex {
    use super::*;

    pub fn serialize<S: Serializer>(
        color: &Option<ColorU>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match color {
            Some(color) if color.a == 255 => {
                serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b))
            }
            Some(color) => serializer.serialize_str(&format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                color.r, color.g, color.b, color.a
            )),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ColorU>, D::Error> {
        let Some(text) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        // `#rrggbbaa` round-trips what `serialize` writes.
        if let Some(hex) = text.strip_prefix('#')
            && hex.len() == 8
        {
            let value = u32::from_str_radix(hex, 16).map_err(serde::de::Error::custom)?;
            return Ok(Some(ColorU::from_u32(value)));
        }
        parse_css_color(&text)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("not a colour: {text}")))
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
