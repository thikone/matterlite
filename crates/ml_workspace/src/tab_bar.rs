// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/tab.rs` and the tab bar of `app/src/workspace/view.rs`
// (`render_tab_bar_contents`, `render_tab_bar_icon_button`) @ 5bb7bcc6a (the `NewTabStyling`
// look: overlay fills, 1-px dividers, the close button over the tab's right end on hover, the
// hover-fixed tab width, the tools panel button); the code is new.

//! The chat tab bar across the top of the window (MatterLite.md §10.7). The app supplies the
//! tabs ([`TabSpec`]); the bar draws them and reports clicks as workspace actions.

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_core::ui::theme::color::internal_colors;
use warpui::Element;
use warpui::color::ColorU;
use warpui::elements::{
    Align, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    DispatchEventResult, Empty, EventHandler, Fill, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};

use crate::buttons::icon_button_with_color;
use crate::{TAB_BAR_HEIGHT, WorkspaceAction};

const TAB_MIN_WIDTH: f32 = 90.;
const TAB_MAX_WIDTH: f32 = 220.;
const PINNED_TAB_MAX_WIDTH: f32 = 150.;
/// Warp's padding either side of a tab's title (equal, so the title stays centred).
const TAB_CONTENT_PADDING: f32 = 8.;
/// A pinned tab's padding: room for the pin at the right, mirrored at the left.
const PINNED_TAB_CONTENT_PADDING: f32 = 26.;
/// Before the first item of the bar (Warp's `compute_tab_bar_left_padding`).
const TAB_BAR_LEADING: f32 = 16.;
/// Between the tab bar's icon buttons (Warp's `TAB_BAR_ICON_PADDING`).
const TAB_BAR_ICON_PADDING: f32 = 4.;
const TAB_BAR_PADDING_RIGHT: f32 = 8.;
const CLOSE_BUTTON_WIDTH: f32 = 20.;
/// The close button's distance from the tab's right edge.
const CLOSE_BUTTON_INSET: f32 = 6.;
/// The close button's fill over the title, in percent of `surface_3`.
const CLOSE_BUTTON_OPACITY: u8 = 60;

/// Shortcut labels for the tab bar's tooltips (the bindings are in `workspace::init`).
const NEW_TAB_KEY_LABEL: &str = if cfg!(target_os = "macos") {
    "⌘K"
} else {
    "Ctrl+K"
};
const TOOL_PANEL_KEY_LABEL: &str = if cfg!(target_os = "macos") {
    "⌘\\"
} else {
    "Ctrl+Shift+\\"
};

fn tab_position_id(index: usize) -> String {
    format!("ml_workspace:tab:{index}")
}
const LEADING_SIZE: f32 = 13.;

/// What a tab shows; supplied by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct TabSpec {
    pub title: String,
    pub leading: TabLeading,
    pub badge: TabBadge,
    /// Drawn bold.
    pub unread: bool,
    pub pinned: bool,
    /// Someone is typing in the chat: "typing…" after the title.
    pub typing: bool,
}

/// The mark left of the title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabLeading {
    None,
    Icon(Icon),
    /// A status dot; hollow when not `filled`.
    Status {
        color: ColorU,
        filled: bool,
    },
    /// A small box with a number (a group chat's member count).
    Count(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabBadge {
    None,
    /// Unread messages without mentions.
    Dot,
    Mentions(i64),
}

/// One chat tab. Its mouse states live here so hover survives re-renders.
pub struct ChatTab {
    pub spec: TabSpec,
    pub(crate) mouse_state: MouseStateHandle,
    pub(crate) close_mouse_state: MouseStateHandle,
}

impl ChatTab {
    pub fn new(spec: TabSpec) -> Self {
        Self {
            spec,
            mouse_state: MouseStateHandle::default(),
            close_mouse_state: MouseStateHandle::default(),
        }
    }
}

/// The right-click menu of a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabMenuItem {
    TogglePin,
    Close,
    CloseOthers,
    Reopen,
}

#[derive(Default)]
pub(crate) struct TabMenu {
    pub index: usize,
    pub mouse_states: [MouseStateHandle; 4],
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

/// Everything the tab bar draws; the workspace fills it in each render.
pub(crate) struct TabBar<'a> {
    pub tabs: &'a [ChatTab],
    pub active: Option<usize>,
    pub menu: Option<&'a TabMenu>,
    /// While the pointer is on a tab's close button, the tabs of its kind (pinned or not) keep
    /// that tab's width, so after a close the next tab's button slides under the pointer
    /// (Warp's `hover_fixed_width`). Pinned tabs are narrower and grouped on the left, so the
    /// other kind keeps its own sizing.
    pub fixed_tab_width: Option<(f32, bool)>,
    pub tool_panel_open: bool,
    pub tool_panel_mouse_state: &'a MouseStateHandle,
    pub new_tab_mouse_state: &'a MouseStateHandle,
    pub account: Option<(&'a AccountBadge, &'a MouseStateHandle)>,
    /// The window's buttons, drawn at the right end (Windows, Linux).
    pub window_buttons: Option<Box<dyn Element>>,
    /// Leave room at the left for the native macOS buttons.
    pub macos_buttons: bool,
}

/// A tab bar icon button as Warp draws one: sub-text colour, main-text colour and a fill while
/// `active`, and a tooltip with the shortcut under the label.
fn tab_bar_icon_button(
    appearance: &Appearance,
    icon: Icon,
    mouse_state: &MouseStateHandle,
    active: bool,
    label: &str,
    shortcut: &str,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let color = if active {
        theme.main_text_color(theme.background())
    } else {
        theme.sub_text_color(theme.background())
    };
    let tooltip = appearance
        .ui_builder()
        .tool_tip_with_sublabel(label.to_owned(), shortcut.to_owned())
        .build()
        .finish();
    let mut button = icon_button_with_color(appearance, icon, active, mouse_state.clone(), color);
    if active {
        button = button.with_active_styles(
            UiComponentStyles::default()
                .set_background(internal_colors::fg_overlay_3(theme).into()),
        );
    }
    button
        .with_tooltip(move || tooltip)
        .build()
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action))
        .with_cursor(Cursor::PointingHand)
        .finish()
}

pub(crate) fn render_tab_bar(bar: TabBar, appearance: &Appearance) -> Box<dyn Element> {
    let TabBar {
        tabs,
        active,
        menu,
        fixed_tab_width,
        tool_panel_open,
        tool_panel_mouse_state,
        new_tab_mouse_state,
        account,
        window_buttons,
        macos_buttons,
    } = bar;
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    let last = tabs.len().saturating_sub(1);
    for (index, tab) in tabs.iter().enumerate() {
        let mut element = SavePosition::new(
            render_tab(tab, index, Some(index) == active, index == last, appearance),
            &tab_position_id(index),
        )
        .finish();
        if let Some(menu) = menu.filter(|menu| menu.index == index) {
            let mut stack = Stack::new().with_child(element);
            stack.add_positioned_overlay_child(
                render_tab_menu(menu, tab.spec.pinned, appearance),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 2.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            );
            element = stack.finish();
        }
        let sized = match fixed_tab_width {
            Some((width, pinned)) if pinned == tab.spec.pinned => {
                ConstrainedBox::new(element).with_width(width)
            }
            _ => ConstrainedBox::new(element)
                .with_min_width(TAB_MIN_WIDTH)
                .with_max_width(if tab.spec.pinned {
                    PINNED_TAB_MAX_WIDTH
                } else {
                    TAB_MAX_WIDTH
                }),
        };
        row.add_child(Shrinkable::new(1., sized.finish()).finish());
    }

    let tool_panel_button = tab_bar_icon_button(
        appearance,
        Icon::Menu,
        tool_panel_mouse_state,
        tool_panel_open,
        "Tools panel",
        TOOL_PANEL_KEY_LABEL,
        WorkspaceAction::ToggleToolPanel,
    );
    let new_tab_button = tab_bar_icon_button(
        appearance,
        Icon::Plus,
        new_tab_mouse_state,
        false,
        "Open a chat",
        NEW_TAB_KEY_LABEL,
        WorkspaceAction::NewTab,
    );

    let mut bar = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(tool_panel_button)
                .with_margin_left(TAB_BAR_ICON_PADDING)
                .with_margin_right(8.)
                .finish(),
        )
        .with_child(Shrinkable::new(1., row.finish()).finish())
        .with_child(
            Container::new(new_tab_button)
                .with_margin_left(TAB_BAR_ICON_PADDING)
                .finish(),
        );
    if let Some((badge, mouse_state)) = account {
        bar.add_child(
            Container::new(render_account_button(badge, mouse_state, appearance))
                .with_margin_left(8.)
                .finish(),
        );
    }
    let bar = bar.finish();

    let padding_left = if macos_buttons {
        crate::window_controls::MACOS_BUTTONS_WIDTH + TAB_BAR_LEADING
    } else {
        TAB_BAR_LEADING
    };
    let bar = Container::new(
        ConstrainedBox::new(bar)
            .with_height(TAB_BAR_HEIGHT)
            .finish(),
    )
    .with_padding_left(padding_left)
    .with_padding_right(TAB_BAR_PADDING_RIGHT)
    .finish();
    // The window buttons sit flush with the window's right edge, like the OS's own.
    let row = match window_buttons {
        Some(buttons) => Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(Shrinkable::new(1., bar).finish())
            .with_child(buttons)
            .finish(),
        None => bar,
    };
    // The mouse's back and forward buttons go to the previous and next tab, as in Warp.
    let row = EventHandler::new(row)
        .on_back_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(WorkspaceAction::ActivatePrevTab);
            DispatchEventResult::StopPropagation
        })
        .on_forward_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(WorkspaceAction::ActivateNextTab);
            DispatchEventResult::StopPropagation
        })
        .finish();
    let theme = appearance.theme();
    Container::new(row)
        .with_border(Border::bottom(1.).with_border_fill(internal_colors::fg_overlay_3(theme)))
        .finish()
}

/// The close button laid over the tab's right end while the tab is hovered. Its translucent
/// fill keeps a long title readable under it. Hovering it fixes the tabs' width
/// (`TabHoverWidthStart`) until the pointer leaves it.
fn render_close_button(
    tab: &ChatTab,
    index: usize,
    is_last: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let pinned = tab.spec.pinned;
    let surface = theme.surface_3().into_solid();
    let mut translucent = surface;
    translucent.a = (u16::from(surface.a) * u16::from(CLOSE_BUTTON_OPACITY) / 100) as u8;
    let styles = |color| {
        UiComponentStyles::default()
            .set_background(Fill::Solid(color))
            .set_border_radius(CornerRadius::with_all(Radius::Pixels(2.)))
    };
    icon_button_with_color(
        appearance,
        Icon::X,
        false,
        tab.close_mouse_state.clone(),
        theme.sub_text_color(theme.background()),
    )
    .with_style(styles(translucent))
    .with_hovered_styles(styles(surface))
    .with_active_styles(styles(surface))
    .build()
    .on_hover(move |hovered, ctx, _, _| {
        if hovered {
            if let Some(rect) = ctx.element_position_by_id(tab_position_id(index)) {
                ctx.dispatch_typed_action(WorkspaceAction::TabHoverWidthStart {
                    width: rect.width(),
                    pinned,
                });
            }
        } else {
            ctx.dispatch_typed_action(WorkspaceAction::TabHoverWidthEnd);
        }
    })
    .on_click(move |ctx, _, _| {
        // No tab slides into the last tab's place, so the others may grow at once.
        if is_last {
            ctx.dispatch_typed_action(WorkspaceAction::TabHoverWidthEnd);
        }
        ctx.dispatch_typed_action(WorkspaceAction::CloseTab(index))
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

fn render_tab(
    tab: &ChatTab,
    index: usize,
    is_active: bool,
    is_last: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme().clone();
    let chat = theme.chat();
    let font = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    let spec = tab.spec.clone();
    let close_button = render_close_button(tab, index, is_last, appearance);

    Hoverable::new(tab.mouse_state.clone(), move |mouse_state| {
        let is_hovered = mouse_state.is_hovered();
        let background: Fill = if is_active {
            internal_colors::fg_overlay_2(&theme).into()
        } else if is_hovered {
            internal_colors::fg_overlay_1(&theme).into()
        } else {
            Fill::None
        };
        let text_color = if is_active || spec.unread {
            theme.main_text_color(theme.background())
        } else {
            theme.sub_text_color(theme.background())
        }
        .into_solid();
        let sub_color = theme.sub_text_color(theme.background()).into_solid();

        let mut content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        if let Some(leading) = render_leading(spec.leading, text_color, font) {
            content.add_child(Container::new(leading).with_margin_right(6.).finish());
        }
        content.add_child(
            Shrinkable::new(
                1.,
                Text::new_inline(spec.title.clone(), font, font_size)
                    .with_color(text_color)
                    .with_style(Properties::default().weight(if spec.unread {
                        Weight::Bold
                    } else {
                        Weight::Normal
                    }))
                    .finish(),
            )
            .finish(),
        );
        if spec.typing {
            content.add_child(
                Container::new(
                    Text::new_inline("typing…", font, font_size - 2.)
                        .with_color(sub_color)
                        .with_style(Properties::default().style(warpui::fonts::Style::Italic))
                        .finish(),
                )
                .with_margin_left(6.)
                .finish(),
            );
        }
        match spec.badge {
            TabBadge::Mentions(count) => {
                let label = if count > 99 {
                    "99+".to_owned()
                } else {
                    count.to_string()
                };
                content.add_child(
                    Container::new(
                        Text::new_inline(label, font, font_size - 2.)
                            .with_color(chat.button_text)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .with_margin_left(6.)
                    .with_horizontal_padding(5.)
                    .with_background_color(chat.button_bg)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.)))
                    .finish(),
                );
            }
            TabBadge::Dot => content.add_child(
                Container::new(dot(text_color, 6.))
                    .with_margin_left(6.)
                    .finish(),
            ),
            TabBadge::None => {}
        }
        // As in Warp, the title is centred in the whole tab and the close button is laid over
        // the tab's right end while the pointer is on the tab. A pinned tab shows a pin there,
        // which turns into the close button under the pointer.
        let padding = if spec.pinned {
            PINNED_TAB_CONTENT_PADDING
        } else {
            TAB_CONTENT_PADDING
        };
        let mut stack = Stack::new().with_child(
            Container::new(content.finish())
                .with_horizontal_padding(padding)
                .with_vertical_padding(2.)
                .finish(),
        );
        let slot = if is_hovered {
            Some(close_button)
        } else if spec.pinned {
            Some(
                Align::new(
                    ConstrainedBox::new(
                        Icon::Pin
                            .to_warpui_icon(ThemeFill::Solid(sub_color))
                            .finish(),
                    )
                    .with_width(12.)
                    .with_height(12.)
                    .finish(),
                )
                .finish(),
            )
        } else {
            None
        };
        if let Some(slot) = slot {
            stack.add_positioned_child(
                ConstrainedBox::new(slot)
                    .with_width(CLOSE_BUTTON_WIDTH)
                    .with_height(CLOSE_BUTTON_WIDTH)
                    .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(-CLOSE_BUTTON_INSET, 0.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::MiddleRight,
                    ChildAnchor::MiddleRight,
                ),
            );
        }

        let border = if is_active {
            internal_colors::fg_overlay_4(&theme)
        } else {
            internal_colors::fg_overlay_3(&theme)
        };
        Container::new(stack.finish())
            .with_background(background)
            .with_border(
                Border::all(1.)
                    // Only the first tab has a left divider, so neighbours don't double up.
                    .with_sides(false, index == 0, false, true)
                    .with_border_fill(border),
            )
            .finish()
    })
    // A click on the close button must not also activate the tab that takes its place.
    .with_defer_events_to_children()
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::ActivateTab(index)))
    .on_middle_click(move |ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::CloseTab(index)))
    .on_right_click(move |ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::OpenTabMenu(index)))
    .with_cursor(Cursor::PointingHand)
    .finish()
}

fn render_leading(
    leading: TabLeading,
    text_color: ColorU,
    font: warpui::fonts::FamilyId,
) -> Option<Box<dyn Element>> {
    Some(match leading {
        TabLeading::None => return None,
        TabLeading::Icon(icon) => {
            ConstrainedBox::new(icon.to_warpui_icon(ThemeFill::Solid(text_color)).finish())
                .with_width(LEADING_SIZE)
                .with_height(LEADING_SIZE)
                .finish()
        }
        TabLeading::Status { color, filled } => {
            let mut container = Container::new(Empty::new().finish())
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)));
            container = if filled {
                container.with_background_color(color)
            } else {
                container.with_border(Border::all(1.5).with_border_color(color))
            };
            ConstrainedBox::new(container.finish())
                .with_width(9.)
                .with_height(9.)
                .finish()
        }
        TabLeading::Count(count) => {
            let mut faded = text_color;
            faded.a /= 5;
            ConstrainedBox::new(
                Container::new(
                    Align::new(
                        Text::new_inline(count.to_string(), font, 9.)
                            .with_color(text_color)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .finish(),
                )
                .with_background_color(faded)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
                .finish(),
            )
            .with_width(LEADING_SIZE)
            .with_height(LEADING_SIZE)
            .finish()
        }
    })
}

fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

fn render_tab_menu(menu: &TabMenu, pinned: bool, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let background = theme.surface_1();
    let text = theme.main_text_color(background).into_solid();
    let hover = theme.surface_2();
    let font = appearance.ui_font_family();
    let size = appearance.ui_font_size();
    let index = menu.index;
    let items = [
        (
            TabMenuItem::TogglePin,
            if pinned { "Unpin tab" } else { "Pin tab" },
        ),
        (TabMenuItem::Close, "Close tab"),
        (TabMenuItem::CloseOthers, "Close other tabs"),
        (TabMenuItem::Reopen, "Reopen closed tab"),
    ];
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for ((item, label), mouse_state) in items.into_iter().zip(&menu.mouse_states) {
        column.add_child(
            Hoverable::new(mouse_state.clone(), move |state| {
                let mut container = Container::new(
                    Text::new_inline(label.to_owned(), font, size)
                        .with_color(text)
                        .finish(),
                )
                .with_horizontal_padding(12.)
                .with_vertical_padding(6.);
                if state.is_hovered() {
                    container = container.with_background(hover);
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::TabMenu { index, item })
            })
            .finish(),
        );
    }
    let menu = Container::new(
        ConstrainedBox::new(column.finish())
            .with_width(180.)
            .finish(),
    )
    .with_vertical_padding(4.)
    .with_background(background)
    .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .finish();
    warpui::elements::Dismiss::new(menu)
        .on_dismiss(|ctx, _| ctx.dispatch_typed_action(WorkspaceAction::CloseTabMenu))
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
    let dot_color = match badge.status {
        ConnectionStatus::Online => theme.terminal_colors().normal.green,
        ConnectionStatus::Connecting => theme.terminal_colors().normal.yellow,
        ConnectionStatus::Offline => theme.terminal_colors().normal.red,
    };
    let tooltip = appearance
        .ui_builder()
        .tool_tip(badge.tooltip.clone())
        .build()
        .finish();
    Hoverable::new(mouse_state.clone(), move |state| {
        let accent = theme.accent();
        let circle = Container::new(
            Align::new(
                Text::new_inline(initials, font, 10.)
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
        if state.is_hovered() {
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
