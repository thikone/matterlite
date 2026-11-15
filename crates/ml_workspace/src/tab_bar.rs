// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/tab.rs` @ 5bb7bcc6a (the `NewTabStyling` look: overlay
// fills, 1-px dividers, close button on hover); the code is new.

//! The chat tab bar across the top of the window (MatterLite.md §10.7).

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warpui::Element;
use warpui::elements::{
    Align, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty,
    Fill, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
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

/// The signed-in account, shown at the right end of the tab bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountBadge {
    /// One or two letters (the user's initials).
    pub initials: String,
    pub tooltip: String,
    pub status: ConnectionStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Online,
    Connecting,
    Offline,
}

const ACCOUNT_BUTTON_SIZE: f32 = 22.;
const STATUS_DOT_SIZE: f32 = 8.;

pub(crate) fn render_tab_bar(
    tabs: &[ChatTab],
    active: usize,
    new_tab_mouse_state: &MouseStateHandle,
    account: Option<(&AccountBadge, &MouseStateHandle)>,
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

    let mut bar = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Shrinkable::new(1., row.finish()).finish())
        .with_child(Container::new(new_tab_button).with_margin_left(4.).finish());
    if let Some((badge, mouse_state)) = account {
        bar.add_child(
            Container::new(render_account_button(badge, mouse_state, appearance))
                .with_margin_left(8.)
                .finish(),
        );
    }
    let bar = bar.finish();

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

/// A round button with the user's initials and a connection dot; opens the account menu.
fn render_account_button(
    badge: &AccountBadge,
    mouse_state: &MouseStateHandle,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme().clone();
    let font = appearance.ui_font_family();
    let initials = badge.initials.clone();
    let tooltip = badge.tooltip.clone();
    let dot_color = match badge.status {
        ConnectionStatus::Online => theme.terminal_colors().normal.green,
        ConnectionStatus::Connecting => theme.terminal_colors().normal.yellow,
        ConnectionStatus::Offline => theme.terminal_colors().normal.red,
    };
    let tooltip_element = appearance.ui_builder().tool_tip(tooltip).build().finish();
    let mut tooltip_element = Some(tooltip_element);
    Hoverable::new(mouse_state.clone(), move |state| {
        let accent = theme.accent();
        let circle = Container::new(
            Align::new(
                Text::new_inline(initials.clone(), font, 10.)
                    .with_color(theme.main_text_color(accent).into_solid())
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            )
            .finish(),
        )
        .with_background(accent)
        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
        .finish();
        let dot = Container::new(Empty::new().finish())
            .with_background_color(dot_color.into())
            .with_border(Border::all(1.5).with_border_fill(theme.background()))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish();
        let mut stack = Stack::new()
            .with_child(
                ConstrainedBox::new(circle)
                    .with_width(ACCOUNT_BUTTON_SIZE)
                    .with_height(ACCOUNT_BUTTON_SIZE)
                    .finish(),
            )
            .with_positioned_child(
                ConstrainedBox::new(dot)
                    .with_width(STATUS_DOT_SIZE)
                    .with_height(STATUS_DOT_SIZE)
                    .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(1., 1.),
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::BottomRight,
                    ChildAnchor::BottomRight,
                ),
            );
        if state.is_hovered()
            && let Some(tooltip) = tooltip_element.take()
        {
            stack.add_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 6.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomRight,
                    ChildAnchor::TopRight,
                ),
            );
        }
        stack.finish()
    })
    .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::OpenAccountMenu))
    .with_cursor(Cursor::PointingHand)
    .finish()
}
