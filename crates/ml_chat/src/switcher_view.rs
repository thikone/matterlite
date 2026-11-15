// SPDX-License-Identifier: AGPL-3.0-only

//! The chat switcher (Ctrl/Cmd+K, MatterLite.md §10.2 `ChannelSwitcher`): a field and the chats
//! matching it across all teams; Enter or a click focuses the chat's tab or opens one.

use ml_editor::{
    EditorView, Event as EditorEvent, NavigationKey, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};
use mm_model::ChannelId;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Expanded, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Radius, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats::{self, SwitcherItem};
use crate::chats_view::{badge, channel_icon};
use crate::session::Session;

const WIDTH: f32 = 520.;
const ROW_HEIGHT: f32 = 32.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherAction {
    Choose(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitcherEvent {
    Chosen(ChannelId),
    Dismissed,
}

pub struct SwitcherView {
    field: ViewHandle<EditorView>,
    items: Vec<SwitcherItem>,
    selected: usize,
    mouse_states: Vec<MouseStateHandle>,
}

impl SwitcherView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        let options = SingleLineEditorOptions {
            text: TextOptions::ui_text(Some(16.), Appearance::as_ref(ctx)),
            propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Always,
            ..Default::default()
        };
        let field = ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx));
        field.update(ctx, |field, ctx| {
            field.set_placeholder_text("Find a chat by name", ctx)
        });
        ctx.subscribe_to_view(&field, |me, _, event, ctx| match event {
            EditorEvent::Edited(_) => me.refresh(ctx),
            EditorEvent::Enter => me.choose(me.selected, ctx),
            EditorEvent::Escape => ctx.emit(SwitcherEvent::Dismissed),
            EditorEvent::Navigate(NavigationKey::Up | NavigationKey::ShiftTab) => {
                me.move_selection(-1, ctx)
            }
            EditorEvent::Navigate(NavigationKey::Down | NavigationKey::Tab) => {
                me.move_selection(1, ctx)
            }
            _ => {}
        });
        let mut view = Self {
            field,
            items: Vec::new(),
            selected: 0,
            mouse_states: (0..chats::SWITCHER_RESULTS)
                .map(|_| MouseStateHandle::default())
                .collect(),
        };
        view.refresh(ctx);
        view
    }

    /// Clears the field and lists the recent chats again (each time it opens).
    pub fn reset(&mut self, ctx: &mut ViewContext<Self>) {
        self.field
            .update(ctx, |field, ctx| field.system_clear_buffer(false, ctx));
        self.refresh(ctx);
    }

    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let query = self.field.as_ref(ctx).buffer_text(ctx);
        self.items = Session::as_ref(ctx)
            .store()
            .and_then(|store| {
                store
                    .read()
                    .ok()
                    .map(|store| chats::switcher_items(&store, &query))
            })
            .unwrap_or_default();
        self.selected = 0;
        ctx.notify();
    }

    fn move_selection(&mut self, delta: isize, ctx: &mut ViewContext<Self>) {
        if !self.items.is_empty() {
            let len = self.items.len() as isize;
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            ctx.notify();
        }
    }

    fn choose(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if let Some(item) = self.items.get(index) {
            ctx.emit(SwitcherEvent::Chosen(item.id.clone()));
        }
    }
}

impl Entity for SwitcherView {
    type Event = SwitcherEvent;
}

impl View for SwitcherView {
    fn ui_name() -> &'static str {
        "SwitcherView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.field);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let chat = theme.chat();
        let font = appearance.ui_font_family();
        let background = theme.surface_1();
        let text = theme.main_text_color(background).into_solid();
        let sub = theme.sub_text_color(background).into_solid();
        let selected_fill = theme.surface_3();

        let field = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(Icon::Search.to_warpui_icon(Fill::Solid(sub)).finish())
                        .with_width(16.)
                        .with_height(16.)
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Container::new(ChildView::new(&self.field).finish())
                            .with_margin_left(8.)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(12.)
        .with_border(Border::bottom(1.).with_border_fill(theme.surface_3()))
        .finish();

        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if self.items.is_empty() {
            list.add_child(
                Container::new(
                    Text::new_inline("No chats match.".to_owned(), font, 13.)
                        .with_color(sub)
                        .finish(),
                )
                .with_uniform_padding(12.)
                .finish(),
            );
        }
        for (index, (item, mouse_state)) in self.items.iter().zip(&self.mouse_states).enumerate() {
            let selected = index == self.selected;
            let unread = item.unread.is_unread();
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(channel_icon(item.icon, text, &chat, font))
                        .with_width(18.)
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Container::new(
                            Text::new_inline(item.name.clone(), font, 14.)
                                .with_color(text)
                                .with_style(Properties::default().weight(if unread {
                                    Weight::Bold
                                } else {
                                    Weight::Normal
                                }))
                                .finish(),
                        )
                        .with_margin_left(8.)
                        .finish(),
                    )
                    .finish(),
                );
            if let Some(team) = &item.team {
                row.add_child(
                    Container::new(
                        Text::new_inline(team.clone(), font, 12.)
                            .with_color(sub)
                            .finish(),
                    )
                    .with_margin_left(8.)
                    .finish(),
                );
            }
            if item.unread.mentions > 0 {
                row.add_child(
                    Container::new(badge(item.unread.mentions, &chat, font))
                        .with_margin_left(8.)
                        .finish(),
                );
            }
            let row = row.finish();
            list.add_child(
                Hoverable::new(mouse_state.clone(), move |state| {
                    let mut container =
                        Container::new(ConstrainedBox::new(row).with_height(ROW_HEIGHT).finish())
                            .with_horizontal_padding(12.);
                    if selected {
                        container = container.with_background(selected_fill);
                    } else if state.is_hovered() {
                        container = container.with_background(theme_hover(selected_fill));
                    }
                    container.finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(SwitcherAction::Choose(index)))
                .finish(),
            );
        }

        Container::new(
            ConstrainedBox::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(field)
                    .with_child(
                        Container::new(list.finish())
                            .with_vertical_padding(6.)
                            .finish(),
                    )
                    .finish(),
            )
            .with_width(WIDTH)
            .finish(),
        )
        .with_background(background)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .finish()
    }
}

/// The hover fill: the selection fill at half strength.
fn theme_hover(selected: Fill) -> Fill {
    selected.with_opacity(50)
}

impl TypedActionView for SwitcherView {
    type Action = SwitcherAction;

    fn handle_action(&mut self, action: &SwitcherAction, ctx: &mut ViewContext<Self>) {
        match action {
            SwitcherAction::Choose(index) => self.choose(*index, ctx),
        }
    }
}
