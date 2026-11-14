// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/notebooks/editor/mod.rs` (`rich_text_styles`, the Markdown
// table style) and `app/src/code/editor/view.rs` (`code_text_styles`) @ 5bb7bcc6a — adapted for
// MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Text styles for the viewers: Markdown documents in the UI font with monospace code, and code
//! files in the monospace font. Derived from the active theme.

use pathfinder_color::ColorU;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::builder::CHECK_SVG_PATH;
use warp_core::ui::color::ContrastingColor as _;
use warp_core::ui::color::contrast::MinimumAllowedContrast;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::render::model::{
    BlockSpacing, BrokenLinkStyle, CheckBoxStyle, HorizontalRuleStyle, InlineCodeStyle,
    ParagraphStyles, RichTextStyles, TableStyle,
};
use warpui::elements::{Border, Margin, Padding};
use warpui::fonts::FamilyId;
use warpui::ui_components::checkbox::HOVER_BACKGROUND_COLOR;

const DOCUMENT_LINE_HEIGHT_RATIO: f32 = 1.5;
const DOCUMENT_BASELINE_RATIO: f32 = 0.7;
/// Warp's default notebook font size.
pub const DOCUMENT_FONT_SIZE: f32 = 14.;

fn markdown_table_style(
    appearance: &Appearance,
    font_family: FamilyId,
    font_size: f32,
) -> TableStyle {
    let theme = appearance.theme();
    TableStyle {
        border_color: internal_colors::neutral_4(theme),
        header_background: ColorU::transparent_black(),
        cell_background: ColorU::transparent_black(),
        alternate_row_background: None,
        text_color: internal_colors::text_sub(theme, theme.background()),
        header_text_color: internal_colors::text_main(theme, theme.background()),
        scrollbar_nonactive_thumb_color: theme.nonactive_ui_detail().into_solid(),
        scrollbar_active_thumb_color: theme.active_ui_detail().into_solid(),
        font_family,
        font_size,
        cell_padding: 12.,
        outer_border: false,
        column_dividers: false,
        row_dividers: true,
    }
}

/// Styles for rendered Markdown and Jupyter documents.
pub fn document_text_styles(appearance: &Appearance) -> RichTextStyles {
    let line_height_ratio = DOCUMENT_LINE_HEIGHT_RATIO;
    let baseline_ratio = DOCUMENT_BASELINE_RATIO;
    let theme = appearance.theme();
    let inline_font_color: ColorU = theme.terminal_colors().normal.red.into();
    let font_size = DOCUMENT_FONT_SIZE;

    let base_text = ParagraphStyles {
        font_size,
        font_weight: Default::default(),
        line_height_ratio,
        font_family: appearance.ui_font_family(),
        text_color: theme.main_text_color(theme.background()).into_solid(),
        baseline_ratio,
        fixed_width_tab_size: None,
    };

    RichTextStyles {
        base_text,
        code_text: ParagraphStyles {
            font_family: appearance.monospace_font_family(),
            font_size,
            font_weight: Default::default(),
            line_height_ratio,
            text_color: theme.main_text_color(theme.background()).into_solid(),
            baseline_ratio,
            fixed_width_tab_size: Some(4),
        },
        code_background: theme.background().into(),
        embedding_background: theme.surface_2().into(),
        embedding_text: ParagraphStyles {
            font_size,
            font_weight: Default::default(),
            line_height_ratio,
            font_family: appearance.monospace_font_family(),
            text_color: theme.main_text_color(theme.surface_2()).into_solid(),
            baseline_ratio,
            fixed_width_tab_size: Some(4),
        },
        code_border: Border::all(1.).with_border_fill(theme.surface_3()),
        placeholder_color: theme.hint_text_color(theme.background()).into_solid(),
        selection_fill: theme.text_selection_color().into(),
        cursor_fill: theme
            .cursor()
            .on_background(theme.background(), MinimumAllowedContrast::Text)
            .into(),
        inline_code_style: InlineCodeStyle {
            font_family: appearance.monospace_font_family(),
            background: theme.surface_3().into(),
            font_color: inline_font_color
                .on_background(theme.surface_3().into(), MinimumAllowedContrast::Text),
        },
        check_box_style: CheckBoxStyle {
            border_color: theme.foreground().into(),
            border_width: 2.,
            icon_path: CHECK_SVG_PATH,
            background: theme.accent().into(),
            hover_background: *HOVER_BACKGROUND_COLOR,
        },
        horizontal_rule_style: HorizontalRuleStyle {
            color: theme.surface_3().into(),
            rule_height: 3.,
        },
        broken_link_style: BrokenLinkStyle {
            icon_path: "bundled/svg/link-broken-02.svg",
            icon_color: theme.terminal_colors().normal.red.into(),
        },
        block_spacings: Default::default(),
        // A viewer never shows "type here" placeholders.
        show_placeholder_text_on_empty_block: false,
        minimum_paragraph_height: Some(base_text.line_height()),
        cursor_width: 3.,
        highlight_urls: true,
        table_style: markdown_table_style(appearance, appearance.ui_font_family(), font_size),
    }
}

/// Styles for code and plain-text files: one monospace paragraph per line.
pub fn code_text_styles(appearance: &Appearance) -> RichTextStyles {
    let mut styling = document_text_styles(appearance);
    let theme = appearance.theme();
    styling.base_text = ParagraphStyles {
        font_size: appearance.monospace_font_size(),
        line_height_ratio: appearance.line_height_ratio(),
        font_family: appearance.monospace_font_family(),
        font_weight: Default::default(),
        text_color: theme.main_text_color(theme.background()).into_solid(),
        baseline_ratio: 0.8,
        fixed_width_tab_size: Some(4),
    };
    styling.block_spacings.text = BlockSpacing {
        margin: Margin::uniform(0.).with_left(1.),
        padding: Padding::uniform(0.),
    };
    styling.minimum_paragraph_height = None;
    styling.cursor_width = 2.;
    // Links in code aren't clickable, so don't highlight them.
    styling.highlight_urls = false;
    styling
}
