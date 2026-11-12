// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/workspace/view/right_panel.rs` @ 5bb7bcc6a (header with
// close button, resizable from the left edge); the code is new.

//! The right panel: reply threads and post edit history (MatterLite.md §10.2).

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DragBarSide, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Resizable, ResizableStateHandle, Shrinkable,
    Text, resizable_state_handle,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::buttons::icon_button_with_color;
use crate::{PANEL_HEADER_HEIGHT, WorkspaceAction};

const DEFAULT_WIDTH: f32 = 380.;
const MIN_WIDTH: f32 = 280.;
const MAX_WIDTH_RATIO: f32 = 0.6;

pub struct RightPanel {
    close_mouse_state: MouseStateHandle,
    resizable_state: ResizableStateHandle,
}

impl RightPanel {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        Self {
            close_mouse_state: MouseStateHandle::default(),
            resizable_state: resizable_state_handle(DEFAULT_WIDTH),
        }
    }
}

impl Entity for RightPanel {
    type Event = ();
}

impl View for RightPanel {
    fn ui_name() -> &'static str {
        "RightPanel"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();

        let close_button = icon_button_with_color(
            appearance,
            Icon::X,
            false,
            self.close_mouse_state.clone(),
            theme.sub_text_color(theme.background()),
        )
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::ToggleRightPanel))
        .with_cursor(Cursor::PointingHand)
        .finish();

        let header = Container::new(
            ConstrainedBox::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Text::new_inline("Thread".to_string(), font, size + 1.)
                            .with_color(theme.main_text_color(theme.background()).into_solid())
                            .finish(),
                    )
                    .with_child(close_button)
                    .finish(),
            )
            .with_height(PANEL_HEADER_HEIGHT)
            .finish(),
        )
        .with_padding_left(12.)
        .with_padding_right(4.)
        .finish();

        let placeholder = Container::new(
            Text::new(
                "Reply threads and edit history open here.".to_string(),
                font,
                size,
            )
            .with_color(theme.hint_text_color(theme.background()).into_solid())
            .finish(),
        )
        .with_uniform_padding(12.)
        .finish();

        let content = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(Shrinkable::new(1., Align::new(placeholder).top_left().finish()).finish())
            .finish();

        Resizable::new(self.resizable_state.clone(), content)
            .with_dragbar_side(DragBarSide::Left)
            .on_resize(|ctx, _| ctx.notify())
            .with_bounds_callback(Box::new(|window_size| {
                (
                    MIN_WIDTH,
                    (window_size.x() * MAX_WIDTH_RATIO).max(MIN_WIDTH),
                )
            }))
            .finish()
    }
}

impl TypedActionView for RightPanel {
    type Action = ();
}
