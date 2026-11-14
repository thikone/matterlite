// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on `render_preview` in warp-terminal `app/src/themes/theme.rs` @ 5bb7bcc6a (thumbnail
// size, background image with opacity, input line with an accent cursor); the content is new: a
// small chat instead of a terminal listing.

//! A thumbnail of a theme for the theme chooser: a channel header, two messages and the
//! composer, drawn in the theme's colours.

use warp_core::ui::theme::WarpTheme;
use warpui::elements::{
    Align, Border, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Empty, Flex, Image,
    ParentElement, Rect, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{Element, color::ColorU};

pub const PREVIEW_WIDTH: f32 = 190.;
pub const PREVIEW_HEIGHT: f32 = 100.;
const TEXT_SIZE: f32 = 9.;
const MARGIN: f32 = 10.;
const LINE_GAP: f32 = 4.;
const AUTHOR_WIDTH: f32 = 34.;

pub fn render_preview(theme: &WarpTheme, font_family: FamilyId) -> Box<dyn Element> {
    let foreground = theme.foreground().into_solid();
    let colors = &theme.terminal_colors().normal;
    let text = |text: &str, color: ColorU| {
        Text::new_inline(text.to_string(), font_family, TEXT_SIZE)
            .with_color(color)
            .finish()
    };
    let message = |author: &str, author_color: ColorU, body: Vec<Box<dyn Element>>| {
        Container::new(
            Flex::row()
                .with_child(
                    ConstrainedBox::new(text(author, author_color))
                        .with_width(AUTHOR_WIDTH)
                        .finish(),
                )
                .with_children(body)
                .finish(),
        )
        .with_margin_top(LINE_GAP)
        .finish()
    };

    let channel = text(
        "# general",
        theme.sub_text_color(theme.background()).into_solid(),
    );
    let first = message(
        "alice",
        colors.blue.into(),
        vec![
            text("Deploy is ", foreground),
            text("done", colors.green.into()),
        ],
    );
    let second = message(
        "bob",
        colors.magenta.into(),
        vec![
            text("@you", theme.accent().into_solid()),
            text(" see the logs", foreground),
        ],
    );

    let composer = Shrinkable::new(
        1.,
        Align::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    Container::new(Empty::new().finish())
                        .with_padding_bottom(5.)
                        .with_border(
                            Border::top(1.2).with_border_color(theme.outline().into_solid()),
                        )
                        .finish(),
                )
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Rect::new()
                                .with_background_color(theme.accent().into_solid())
                                .finish(),
                        )
                        .with_height(11.)
                        .with_width(2.)
                        .finish(),
                    )
                    .with_margin_left(MARGIN)
                    .finish(),
                )
                .finish(),
        )
        .bottom_left()
        .finish(),
    )
    .finish();

    let content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Flex::column()
                    .with_child(channel)
                    .with_child(first)
                    .with_child(second)
                    .finish(),
            )
            .with_margin_left(MARGIN)
            .with_margin_right(MARGIN)
            .finish(),
        )
        .with_child(composer)
        .finish();

    let mut thumbnail = Stack::new();
    let mut background_opacity = 100;
    if let Some(image) = theme.background_image() {
        thumbnail.add_child(
            Shrinkable::new(
                1.,
                Image::new(image.source(), CacheOption::BySize)
                    .cover()
                    .finish(),
            )
            .finish(),
        );
        background_opacity -= image.opacity;
    }
    thumbnail.add_child(
        Container::new(content)
            .with_padding_top(MARGIN - 2.)
            .with_padding_bottom(MARGIN - 2.)
            .with_background(theme.background().with_opacity(background_opacity))
            .finish(),
    );

    Align::new(
        ConstrainedBox::new(thumbnail.finish())
            .with_height(PREVIEW_HEIGHT)
            .with_width(PREVIEW_WIDTH)
            .finish(),
    )
    .finish()
}
