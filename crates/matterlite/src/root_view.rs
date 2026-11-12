// SPDX-License-Identifier: AGPL-3.0-only

use ml_themes::{Appearance, ThemeSettings};
use settings::Setting as _;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Flex, MainAxisAlignment, ParentElement,
    Rect, Stack, Text,
};
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

/// Placeholder window content until the workspace shell (`ml_workspace`) lands.
pub struct RootView;

impl RootView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        Self
    }
}

impl Entity for RootView {
    type Event = ();
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "MatterLiteRootView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let background = theme.background();
        let font = appearance.ui_font_family();
        let theme_name = ThemeSettings::as_ref(app).theme_kind.value().to_string();

        let line = |text: String, size: f32, color| {
            Container::new(
                Text::new_inline(text, font, size)
                    .with_color(color)
                    .finish(),
            )
            .with_margin_bottom(8.)
            .finish()
        };
        let content = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(line(
                "MatterLite".to_string(),
                28.,
                theme.main_text_color(background).into_solid(),
            ))
            .with_child(line(
                "Native Mattermost client. M0 skeleton: WarpUI is running.".to_string(),
                14.,
                theme.sub_text_color(background).into_solid(),
            ))
            .with_child(line(
                format!("Theme: {theme_name}"),
                14.,
                theme.accent().into_solid(),
            ))
            .finish();

        Stack::new()
            .with_child(Rect::new().with_background(background).finish())
            .with_child(
                Align::new(ConstrainedBox::new(content).with_max_width(600.).finish()).finish(),
            )
            .finish()
    }
}

impl TypedActionView for RootView {
    type Action = ();
}
