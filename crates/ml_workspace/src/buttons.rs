// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/ui_components/buttons.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Warp-styled icon buttons (toolbelt, tab bar, panel headers).

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::{ICON_DIMENSIONS, Icon};
use warp_core::ui::theme::{Fill, WarpTheme};
use warpui::elements::{CornerRadius, MouseStateHandle, Radius};
use warpui::ui_components::button::Button;
use warpui::ui_components::components::{Coords, UiComponent as _, UiComponentStyles};

const ICON_BUTTON_PADDING: f32 = 4.;
const BORDER_RADIUS: f32 = 4.;

#[derive(Copy, Clone)]
enum ButtonState {
    Default,
    Disabled,
    Pressed,
    Hover,
}

fn icon_button_styles(warp_theme: &WarpTheme, state: ButtonState) -> UiComponentStyles {
    let (background_color, border_color): (Option<Fill>, Option<Fill>) = match state {
        ButtonState::Default => (None, None),
        ButtonState::Hover => (Some(warp_theme.surface_2()), Some(warp_theme.surface_3())),
        ButtonState::Pressed | ButtonState::Disabled => {
            (Some(warp_theme.background()), Some(warp_theme.surface_3()))
        }
    };

    let mut styles = UiComponentStyles::default()
        .set_width(ICON_DIMENSIONS)
        .set_height(ICON_DIMENSIONS)
        .set_border_width(0.)
        .set_padding(Coords::uniform(ICON_BUTTON_PADDING))
        .set_border_radius(CornerRadius::with_all(Radius::Pixels(BORDER_RADIUS)))
        .set_font_color(warp_theme.foreground().into());

    if let Some(border_color) = border_color {
        styles = styles.set_border_color(border_color.into());
    }
    if let Some(background_color) = background_color {
        styles = styles.set_background(background_color.into());
    }
    styles
}

/// A square icon button with Warp's hover/pressed styling, drawn in `color`.
pub fn icon_button_with_color(
    appearance: &Appearance,
    icon: Icon,
    active: bool,
    mouse_state_handle: MouseStateHandle,
    color: Fill,
) -> Button {
    let theme = appearance.theme();
    let button = Button::new(
        mouse_state_handle,
        icon_button_styles(theme, ButtonState::Default),
        Some(icon_button_styles(theme, ButtonState::Hover)),
        Some(icon_button_styles(theme, ButtonState::Pressed)),
        Some(icon_button_styles(theme, ButtonState::Disabled)),
    )
    .with_icon_label(icon.to_warpui_icon(color))
    // The font color must match too, or the icon is tinted with the default color.
    .with_style(UiComponentStyles::default().set_font_color(color.into()));

    if active { button.active() } else { button }
}
