// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/tab.rs` @ 5bb7bcc6a (the `NewTabStyling` look: overlay
// fills, 1-px dividers, close button on hover); the code is new.

//! The chat tab bar across the top of the window (MatterLite.md §10.7).

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warpui::Element;
use warpui::elements::{
    Border, ConstrainedBox, Container, CrossAxisAlignment, Empty, Fill, Flex, Hoverable,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Shrinkable, Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;

use crate::buttons::icon_button_with_color;
use crate::{TAB_BAR_HEIGHT, WorkspaceAction};

const TAB_MIN_WIDTH: f32 = 90.;
const TAB_MAX_WIDTH: f32 = 220.;
const TAB_HORIZONTAL_PADDING: f32 = 10.;
const TAB_BAR_PADDING_LEFT: f32 = 4.;
const TAB_BAR_PADDING_RIGHT: f32 = 8.;
const CLOSE_BUTTON_WIDTH: f32 = 20.;

/// One chat tab. Its mouse states live here so hover survives re-renders.
pub struct ChatTab {
    pub title: String,
    pub(crate) mouse_state: MouseStateHandle,
    pub(crate) close_mouse_state: MouseStateHandle,
}

impl ChatTab {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            mouse_state: MouseStateHandle::default(),
            close_mouse_state: MouseStateHandle::default(),
        }
    }
}

pub(crate) fn render_tab_bar(
    tabs: &[ChatTab],
    active: usize,
    new_tab_mouse_state: &MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, tab) in tabs.iter().enumerate() {
        row.add_child(
            Shrinkable::new(1., render_tab(tab, index, index == active, appearance)).finish(),
        );
    }

    let new_tab_tooltip = appearance
        .ui_builder()
        .tool_tip("New tab".to_string())
        .build()
        .finish();
    let new_tab_button = icon_button_with_color(
        appearance,
        Icon::Plus,
        false,
        new_tab_mouse_state.clone(),
        theme.sub_text_color(theme.background()),
    )
    .with_tooltip(move || new_tab_tooltip)
    .build()
    .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::NewTab))
    .with_cursor(Cursor::PointingHand)
    .finish();

    let bar = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Shrinkable::new(1., row.finish()).finish())
        .with_child(Container::new(new_tab_button).with_margin_left(4.).finish())
        .finish();

    Container::new(
        ConstrainedBox::new(bar)
            .with_height(TAB_BAR_HEIGHT)
            .finish(),
    )
    .with_padding_left(TAB_BAR_PADDING_LEFT)
    .with_padding_right(TAB_BAR_PADDING_RIGHT)
    .with_border(Border::bottom(1.).with_border_fill(internal_colors::fg_overlay_3(theme)))
    .finish()
}

fn render_tab(
    tab: &ChatTab,
    index: usize,
    is_active: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme().clone();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    let title = tab.title.clone();
    let close_button = icon_button_with_color(
        appearance,
        Icon::X,
        false,
        tab.close_mouse_state.clone(),
        theme.sub_text_color(theme.background()),
    )
    .build()
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::CloseTab(index)))
    .with_cursor(Cursor::PointingHand)
    .finish();

    Hoverable::new(tab.mouse_state.clone(), move |mouse_state| {
        let is_hovered = mouse_state.is_hovered();
        let background: Fill = if is_active {
            internal_colors::fg_overlay_2(&theme).into()
        } else if is_hovered {
            internal_colors::fg_overlay_1(&theme).into()
        } else {
            Fill::None
        };
        let text_color = if is_active {
            theme.main_text_color(theme.background())
        } else {
            theme.sub_text_color(theme.background())
        };

        let mut content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.,
                    Text::new_inline(title.clone(), font, font_size)
                        .with_color(text_color.into_solid())
                        .finish(),
                )
                .finish(),
            );
        // Reserve the close button's space on every tab so titles don't shift on hover.
        content.add_child(if is_active || is_hovered {
            ConstrainedBox::new(close_button)
                .with_width(CLOSE_BUTTON_WIDTH)
                .with_height(CLOSE_BUTTON_WIDTH)
                .finish()
        } else {
            ConstrainedBox::new(Empty::new().finish())
                .with_width(CLOSE_BUTTON_WIDTH)
                .finish()
        });

        let border = if is_active {
            internal_colors::fg_overlay_4(&theme)
        } else {
            internal_colors::fg_overlay_3(&theme)
        };
        ConstrainedBox::new(
            Container::new(content.finish())
                .with_padding_left(TAB_HORIZONTAL_PADDING)
                .with_padding_right(TAB_HORIZONTAL_PADDING / 2.)
                .with_vertical_padding(2.)
                .with_background(background)
                .with_border(
                    Border::all(1.)
                        // Only the first tab has a left divider, so neighbours don't double up.
                        .with_sides(false, index == 0, false, true)
                        .with_border_fill(border),
                )
                .finish(),
        )
        .with_min_width(TAB_MIN_WIDTH)
        .with_max_width(TAB_MAX_WIDTH)
        .finish()
    })
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::ActivateTab(index)))
    .with_cursor(Cursor::PointingHand)
    .finish()
}
