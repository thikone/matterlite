// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/fonts/text_layout_system.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use std::ops::Range;

use crate::fonts::{FontFallbackCache, RequestedFallbackFontSource};
use crate::platform;
use crate::platform::LineStyle;
use crate::text_layout::{ClipConfig, Line, StyleAndFont, TextAlignment, TextFrame};

/// Struct to layout text, updating cached font fallback state as needed.
/// See [fonts::Cache::text_layout_system].
pub struct TextLayoutSystem<'a> {
    pub(super) platform: &'a dyn platform::TextLayoutSystem,
    pub(super) cache: &'a FontFallbackCache,
}

impl TextLayoutSystem<'_> {
    /// Checks if the application specified a fallback font for the given char.
    /// If yes, the UI framework will lazy load the fallback font and trigger
    /// a re-render of the window.
    pub(crate) fn request_fallback_font_for_char(
        &self,
        ch: char,
        source: RequestedFallbackFontSource,
    ) {
        self.cache.request_fallback_font_for_char(ch, source)
    }

    pub fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        clip_config: ClipConfig,
    ) -> Line {
        self.platform
            .layout_line(text, line_style, style_runs, max_width, clip_config)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout_text(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        max_height: f32,
        alignment: TextAlignment,
        first_line_head_indent: Option<f32>,
    ) -> TextFrame {
        self.platform.layout_text(
            text,
            line_style,
            style_runs,
            max_width,
            max_height,
            alignment,
            first_line_head_indent,
        )
    }
}
