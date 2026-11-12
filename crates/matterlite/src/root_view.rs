// SPDX-License-Identifier: AGPL-3.0-only

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Flex, MainAxisAlignment, ParentElement,
    Rect, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::assets;

const BACKGROUND: ColorU = ColorU {
    r: 0x1e,
    g: 0x20,
    b: 0x26,
    a: 0xff,
};
const FOREGROUND: ColorU = ColorU {
    r: 0xdd,
    g: 0xdf,
    b: 0xe4,
    a: 0xff,
};
const MUTED: ColorU = ColorU {
    r: 0x8b,
    g: 0x8f,
    b: 0x99,
    a: 0xff,
};

/// Placeholder window content until the workspace shell (`ml_workspace`) lands.
pub struct RootView {
    font_family: FamilyId,
}

impl RootView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let font_family = warpui::fonts::Cache::handle(ctx).update(ctx, |cache, _| {
            cache
                .load_family_from_bytes(
                    "Roboto",
                    assets::files_with_prefix("bundled/fonts/roboto/"),
                )
                .expect("bundled Roboto font should load")
        });
        Self { font_family }
    }

    fn line(&self, text: &str, size: f32, color: ColorU) -> Box<dyn Element> {
        Container::new(
            Text::new_inline(text.to_string(), self.font_family, size)
                .with_color(color)
                .finish(),
        )
        .with_margin_bottom(8.)
        .finish()
    }
}

impl Entity for RootView {
    type Event = ();
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "MatterLiteRootView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let content = Flex::column()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(self.line("MatterLite", 28., FOREGROUND))
            .with_child(self.line(
                "Native Mattermost client. M0 skeleton: WarpUI is running.",
                14.,
                MUTED,
            ))
            .finish();

        Stack::new()
            .with_child(Rect::new().with_background_color(BACKGROUND).finish())
            .with_child(
                Align::new(ConstrainedBox::new(content).with_max_width(600.).finish()).finish(),
            )
            .finish()
    }
}

impl TypedActionView for RootView {
    type Action = ();
}
