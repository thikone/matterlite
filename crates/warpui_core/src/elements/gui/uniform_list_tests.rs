// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/elements/gui/uniform_list_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pathfinder_geometry::vector::vec2f;

use super::*;
use crate::elements::{
    ChildAnchor, ConstrainedBox, DispatchEventResult, EventHandler, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Rect, Stack,
};
use crate::platform::WindowStyle;
use crate::{
    App, AppContext, Entity, EntityIdSet, Presenter, TypedActionView, ViewContext,
    WindowInvalidation,
};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
enum ElementIdentifier {
    Base,
    Inset,
    Overlay,
}

#[derive(Default)]
struct View {
    // Maps identifier to number of mouse down events
    mouse_downs: HashMap<ElementIdentifier, usize>,
    list_state: UniformListState,
}

pub fn init(app: &mut AppContext) {
    app.add_action("event_handler_test:mouse_down", View::mouse_down);
}

impl View {
    fn mouse_down(&mut self, identifier: &ElementIdentifier, _: &mut ViewContext<Self>) -> bool {
        let entry = self.mouse_downs.entry(*identifier).or_insert(0);
        *entry += 1;
        true
    }
}

impl Entity for View {
    type Event = ();
}

impl crate::core::View for View {
    fn ui_name() -> &'static str {
        "event_handler_test_view"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        UniformList::new(self.list_state.clone(), 1, move |_, _| {
            let mut inner_stack = Stack::new();
            inner_stack.add_child(
                ConstrainedBox::new(Rect::new().finish())
                    .with_height(100.)
                    .with_width(100.)
                    .finish(),
            );
            inner_stack.add_positioned_child(
                EventHandler::new(
                    ConstrainedBox::new(Rect::new().finish())
                        .with_height(25.)
                        .with_width(25.)
                        .finish(),
                )
                .on_left_mouse_down(|evt, _, _| {
                    evt.dispatch_action("event_handler_test:mouse_down", ElementIdentifier::Inset);
                    DispatchEventResult::StopPropagation
                })
                .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 75.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );

            let mut stack = Stack::new();
            stack.add_child(
                EventHandler::new(inner_stack.finish())
                    .on_left_mouse_down(|evt, _, _| {
                        evt.dispatch_action(
                            "event_handler_test:mouse_down",
                            ElementIdentifier::Base,
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
            stack.add_positioned_child(
                EventHandler::new(
                    ConstrainedBox::new(Rect::new().finish())
                        .with_height(25.)
                        .with_width(25.)
                        .finish(),
                )
                .on_left_mouse_down(|evt, _, _| {
                    evt.dispatch_action(
                        "event_handler_test:mouse_down",
                        ElementIdentifier::Overlay,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(75., 0.),
                    ParentOffsetBounds::ParentByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );

            [stack.finish()].into_iter()
        })
        .finish()
    }
}

impl TypedActionView for View {
    type Action = ();
}

#[test]
fn test_uniform_layered_click_handling() {
    App::test((), |mut app| async move {
        let app = &mut app;
        app.update(init);
        let (window_id, view) = app.add_window(WindowStyle::NotStealFocus, |_| View::default());

        let mut presenter = Presenter::new(window_id);

        let mut updated = EntityIdSet::default();
        updated.insert(app.root_view_id(window_id).unwrap());
        let invalidation = WindowInvalidation {
            updated,
            ..Default::default()
        };

        app.update(move |ctx| {
            presenter.invalidate(invalidation, ctx);
            let scene = presenter.build_scene(vec2f(100., 100.), 1., None, ctx);
            assert_eq!(scene.z_index(), ZIndex::new(0));
            assert_eq!(scene.layer_count(), 6);
            let presenter = Rc::new(RefCell::new(presenter));

            // Click on the overlay
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(90., 10.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the inset
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(10., 90.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the top-left area of the base
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(10., 10.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter.clone(),
            );

            // Click on the bottom-right area of the base
            ctx.simulate_window_event(
                Event::LeftMouseDown {
                    position: vec2f(90., 90.),
                    modifiers: Default::default(),
                    click_count: 1,
                    is_first_mouse: false,
                },
                window_id,
                presenter,
            );
        });

        view.read(app, |view, _| {
            assert_eq!(
                1,
                *view.mouse_downs.get(&ElementIdentifier::Overlay).unwrap()
            );
            assert_eq!(1, *view.mouse_downs.get(&ElementIdentifier::Inset).unwrap());
            assert_eq!(2, *view.mouse_downs.get(&ElementIdentifier::Base).unwrap());
        });
    });
}

/// MatterLite: an empty list never asks its builder for an item (builders index their data,
/// and the Chats list did, which crashed the first login on a fresh install).
struct EmptyListView {
    list_state: UniformListState,
    /// In a `Scrollable`, as the Chats list is: the list has a height but nothing to scroll.
    scrollable: Option<crate::elements::ScrollStateHandle>,
}

impl Entity for EmptyListView {
    type Event = ();
}

impl crate::core::View for EmptyListView {
    fn ui_name() -> &'static str {
        "empty_uniform_list_test_view"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let rows: Vec<&str> = Vec::new();
        let list = UniformList::new(self.list_state.clone(), rows.len(), move |range, _| {
            let rows = rows.clone();
            range.map(move |index| {
                let _ = rows[index];
                Rect::new().finish()
            })
        });
        match &self.scrollable {
            Some(state) => crate::elements::Scrollable::vertical(
                state.clone(),
                list.finish_scrollable(),
                crate::elements::ScrollbarWidth::Auto,
                crate::elements::Fill::None,
                crate::elements::Fill::None,
                crate::elements::Fill::None,
            )
            .finish(),
            None => list.finish(),
        }
    }
}

impl TypedActionView for EmptyListView {
    type Action = ();
}

fn build_empty_list(scrollable: bool) {
    App::test((), |mut app| async move {
        let app = &mut app;
        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |_| EmptyListView {
            list_state: UniformListState::new(),
            scrollable: scrollable.then(Default::default),
        });
        let root = app.root_view_id(window_id).unwrap();
        app.update(move |ctx| {
            let mut presenter = Presenter::new(window_id);
            let mut updated = EntityIdSet::default();
            updated.insert(root);
            presenter.invalidate(
                WindowInvalidation {
                    updated,
                    ..Default::default()
                },
                ctx,
            );
            presenter.build_scene(vec2f(200., 300.), 1., None, ctx);
        });
    });
}

#[test]
fn an_empty_list_builds_no_items() {
    build_empty_list(false);
}

/// MatterLite: the scrollbar of an empty list has no thumb; it was placed at 0/0 = NaN, which
/// a debug build's scene asserts on (the Chats list right after the first login).
#[test]
fn an_empty_scrollable_list_paints() {
    build_empty_list(true);
}
