// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/util/traffic_lights.rs`,
// `app/src/util/traffic_lights/windows/renderer.rs` and `renderer_state.rs` @ 5bb7bcc6a — adapted
// for MatterLite.

//! The window's minimize, maximize/restore and close buttons ("traffic lights"). The window has
//! no OS title bar: the tab bar is the title bar (a click on its empty part drags the window, a
//! double-click maximizes; WarpUI's event loop does that). On Windows the buttons are drawn at
//! the right end with the system's symbol font (Segoe Fluent Icons, else Segoe MDL2 Assets),
//! as other Windows apps draw them, or with drawn icons if neither font is there; on Linux
//! as round buttons; on macOS the native buttons are shown at the left and space is left for
//! them. Nothing is drawn when the window has the OS frame (or under a tiling window manager).

use pathfinder_color::ColorU;
use warpui::elements::MainAxisSize;
use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Flex, Hoverable,
    MouseStateHandle, ParentElement, Radius, Rect, Stack,
};
#[cfg(target_os = "windows")]
use warpui::elements::{MainAxisAlignment, Text};
use warpui::fonts::FamilyId;
use warpui::platform::FullscreenState;
use warpui::{AppContext, Element, Entity, ModelContext, SingletonEntity, WindowId};

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::{Fill, WarpTheme};

use crate::TAB_BAR_HEIGHT;

/// What a window button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButton {
    Minimize,
    /// Maximize, or restore a maximized window (leave full screen when full screen).
    Maximize,
    Close,
}

/// The red of the close button under the mouse on Windows.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_BRIGHT_RED: ColorU = ColorU {
    r: 232,
    g: 17,
    b: 32,
    a: u8::MAX,
};
/// The width of each button on Windows, as the OS draws them.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_BUTTON_WIDTH: f32 = 46.;
/// The symbol font's size, and its line height (the golden ratio centres the glyphs).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_ICON_FONT_SIZE: f32 = 10.;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const GOLDEN_RATIO: f32 = 1.618_034;
/// Round buttons on Linux.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_BUTTON_SIZE: f32 = 22.;
/// Space left at the start of the tab bar for the native macOS buttons.
pub const MACOS_BUTTONS_WIDTH: f32 = 72.;

/// Where the buttons are, so the tab bar can leave room for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonsSide {
    /// Drawn by MatterLite at the right end (Windows, Linux).
    Right,
    /// The native buttons at the left (macOS).
    Left,
    /// The OS frame has them, or a tiling window manager has none.
    None,
}

pub fn buttons_side(app: &AppContext, window: WindowId) -> ButtonsSide {
    if app
        .windows()
        .platform_window(window)
        .is_some_and(|window| window.uses_native_window_decorations())
    {
        return ButtonsSide::None;
    }
    if cfg!(target_os = "macos") {
        ButtonsSide::Left
    } else if cfg!(target_os = "linux") && app.windows().is_tiling_window_manager() {
        ButtonsSide::None
    } else {
        ButtonsSide::Right
    }
}

/// Mouse states the view drawing the buttons keeps (created once, so hover works).
#[derive(Clone, Default)]
pub struct WindowButtonStates {
    pub minimize: MouseStateHandle,
    pub maximize: MouseStateHandle,
    pub close: MouseStateHandle,
}

/// Window-wide state for the buttons: the Windows symbol font, and a signal when the window
/// was resized (maximized or restored) so the maximize/restore icon is redrawn.
pub struct WindowFrame {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    icon_font: Option<FamilyId>,
}

#[derive(Clone, Copy, Debug)]
pub enum WindowFrameEvent {
    Resized,
}

impl WindowFrame {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self {
            icon_font: load_icon_font(ctx),
        }
    }

    /// Call from the app's `on_window_resized`.
    pub fn window_resized(&mut self, ctx: &mut ModelContext<Self>) {
        ctx.emit(WindowFrameEvent::Resized);
    }
}

impl Entity for WindowFrame {
    type Event = WindowFrameEvent;
}

impl SingletonEntity for WindowFrame {}

/// "Segoe Fluent Icons" ships with Windows 11, "Segoe MDL2 Assets" with Windows 10.
#[cfg(target_os = "windows")]
fn load_icon_font(ctx: &mut AppContext) -> Option<FamilyId> {
    warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
        ["Segoe Fluent Icons", "Segoe MDL2 Assets"]
            .into_iter()
            .find_map(|name| match cache.get_or_load_system_font(name) {
                Ok(family) => Some(family),
                Err(error) => {
                    log::info!("window buttons: no {name} font ({error})");
                    None
                }
            })
    })
}

#[cfg(not(target_os = "windows"))]
fn load_icon_font(_: &mut AppContext) -> Option<FamilyId> {
    None
}

/// The buttons for the right end of the tab bar (or the login screen), `None` where
/// MatterLite doesn't draw them. `on_click` dispatches the view's action for a button.
pub fn render(
    window: WindowId,
    states: &WindowButtonStates,
    on_click: impl Fn(WindowButton, &mut warpui::EventContext) + Clone + 'static,
    app: &AppContext,
) -> Option<Box<dyn Element>> {
    if buttons_side(app, window) != ButtonsSide::Right {
        return None;
    }
    let fullscreen = app
        .windows()
        .platform_window(window)
        .map_or(FullscreenState::Normal, |window| window.fullscreen_state());
    let theme = Appearance::as_ref(app).theme();
    Some(render_platform(fullscreen, states, on_click, theme, app))
}

/// Shows the native buttons (macOS); a no-op elsewhere.
pub fn show_native_buttons(app: &AppContext, window: WindowId) {
    #[cfg(target_os = "macos")]
    {
        use warpui::platform::mac::WindowExt as _;
        if let Some(platform_window) = app.windows().platform_window(window) {
            platform_window.as_ref().set_window_buttons(true);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, window);
}

#[cfg(target_os = "windows")]
fn render_platform(
    fullscreen: FullscreenState,
    states: &WindowButtonStates,
    on_click: impl Fn(WindowButton, &mut warpui::EventContext) + Clone + 'static,
    theme: &WarpTheme,
    app: &AppContext,
) -> Box<dyn Element> {
    let font = WindowFrame::as_ref(app).icon_font;
    let maximized = fullscreen != FullscreenState::Normal;
    let buttons = [
        (WindowButton::Minimize, &states.minimize, "\u{e921}"),
        (
            WindowButton::Maximize,
            &states.maximize,
            if maximized { "\u{e923}" } else { "\u{e922}" },
        ),
        (WindowButton::Close, &states.close, "\u{e8bb}"),
    ];
    let foreground = theme.foreground().into_solid();
    let hover_background = theme.surface_3();
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_main_axis_alignment(MainAxisAlignment::End)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (button, state, glyph) in buttons {
        let on_click = on_click.clone();
        let close = button == WindowButton::Close;
        let hoverable = Hoverable::new(state.clone(), move |mouse| {
            let hovered = mouse.is_hovered();
            let color = if hovered && close {
                ColorU::white()
            } else {
                foreground
            };
            let icon = match font {
                Some(font) => Text::new(glyph, font, WINDOWS_ICON_FONT_SIZE)
                    .with_color(color)
                    .with_line_height_ratio(GOLDEN_RATIO)
                    .finish(),
                None => drawn_icon(button, maximized, color),
            };
            let icon = Align::new(icon).finish();
            if hovered {
                let background: Fill = if close {
                    WINDOWS_BRIGHT_RED.into()
                } else {
                    hover_background
                };
                Container::new(icon).with_background(background).finish()
            } else {
                icon
            }
        })
        .on_click(move |ctx, _, _| on_click(button, ctx))
        .finish();
        row.add_child(
            ConstrainedBox::new(hoverable)
                .with_width(WINDOWS_BUTTON_WIDTH)
                .finish(),
        );
    }
    ConstrainedBox::new(row.finish())
        .with_height(TAB_BAR_HEIGHT)
        .finish()
}

#[cfg(not(target_os = "windows"))]
fn render_platform(
    fullscreen: FullscreenState,
    states: &WindowButtonStates,
    on_click: impl Fn(WindowButton, &mut warpui::EventContext) + Clone + 'static,
    theme: &WarpTheme,
    _: &AppContext,
) -> Box<dyn Element> {
    let foreground = theme.foreground().into_solid();
    let maximized = fullscreen != FullscreenState::Normal;
    let (normal, hovered) = (theme.surface_2(), theme.surface_3());
    // As wide as the buttons: the caller places the row (an `Align` here would take the
    // whole width and centre them).
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(16.)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);
    for (button, state) in [
        (WindowButton::Minimize, &states.minimize),
        (WindowButton::Maximize, &states.maximize),
        (WindowButton::Close, &states.close),
    ] {
        let on_click = on_click.clone();
        row.add_child(
            Hoverable::new(state.clone(), move |mouse| {
                Container::new(
                    ConstrainedBox::new(
                        Align::new(drawn_icon(button, maximized, foreground)).finish(),
                    )
                    .with_width(LINUX_BUTTON_SIZE)
                    .with_height(LINUX_BUTTON_SIZE)
                    .finish(),
                )
                .with_background(if mouse.is_hovered() { hovered } else { normal })
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .finish()
            })
            .on_click(move |ctx, _, _| on_click(button, ctx))
            .finish(),
        );
    }
    Container::new(row.finish())
        .with_horizontal_padding(12.)
        .with_vertical_padding((TAB_BAR_HEIGHT - LINUX_BUTTON_SIZE) / 2.)
        .finish()
}

/// Icons drawn with rectangles (Linux, and Windows without its symbol fonts).
fn drawn_icon(button: WindowButton, maximized: bool, color: ColorU) -> Box<dyn Element> {
    match button {
        WindowButton::Minimize => {
            ConstrainedBox::new(Rect::new().with_background_color(color).finish())
                .with_height(1.)
                .with_width(10.)
                .finish()
        }
        WindowButton::Maximize => {
            let square = || {
                ConstrainedBox::new(
                    Rect::new()
                        .with_border(Border::all(1.).with_border_color(color))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.)))
                        .finish(),
                )
                .with_width(if maximized { 8. } else { 10. })
                .with_height(if maximized { 8. } else { 10. })
                .finish()
            };
            if !maximized {
                return square();
            }
            // Restore: two overlapping squares.
            let mut stack = Stack::new();
            stack.add_child(Container::new(square()).with_margin_left(2.).finish());
            stack.add_child(
                Container::new(square())
                    .with_margin_top(2.)
                    .with_background_color(ColorU::transparent_black())
                    .finish(),
            );
            ConstrainedBox::new(stack.finish())
                .with_width(10.)
                .with_height(10.)
                .finish()
        }
        WindowButton::Close => {
            ConstrainedBox::new(Icon::X.to_warpui_icon(Fill::Solid(color)).finish())
                .with_width(14.)
                .with_height(14.)
                .finish()
        }
    }
}
