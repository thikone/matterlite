// SPDX-License-Identifier: AGPL-3.0-only

use warpui::elements::{ConstrainedBox, Rect};
use warpui::platform::WindowStyle;
use warpui::{App, Entity, EntityIdSet, Presenter, TypedActionView, View, WindowInvalidation};

use super::*;

const ROW: f32 = 50.;
const VIEWPORT: f32 = 300.;

struct ListView {
    state: TimelineState,
}

impl Entity for ListView {
    type Event = ();
}

impl View for ListView {
    fn ui_name() -> &'static str {
        "TimelineListTest"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let list = TimelineList::new(self.state.clone(), |_, _| {
            ConstrainedBox::new(Rect::new().finish())
                .with_height(ROW)
                .finish()
        });
        ConstrainedBox::new(list.finish())
            .with_height(VIEWPORT)
            .with_width(200.)
            .finish()
    }
}

impl TypedActionView for ListView {
    type Action = ();
}

fn keys(range: std::ops::Range<usize>) -> Vec<(RowKey, u64)> {
    range
        .map(|n| (RowKey::Post(format!("p{n:03}").into()), 0))
        .collect()
}

fn key(n: usize) -> RowKey {
    RowKey::Post(format!("p{n:03}").into())
}

/// Lays the window out and returns what is on screen.
fn layout(app: &mut warpui::App, window_id: warpui::WindowId, state: &TimelineState) -> Viewport {
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
        presenter.build_scene(vec2f(200., VIEWPORT), 1., None, ctx);
    });
    // Reports are sent only when something changed; the last one is what is on screen.
    state.0.borrow().reported.unwrap_or_default()
}

#[test]
fn stays_at_the_bottom_and_keeps_its_place_when_older_rows_arrive() {
    App::test((), |mut app| async move {
        let app = &mut app;
        let (state, _viewports) = TimelineState::new();
        state.set_rows(keys(100..120));
        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, {
            let state = state.clone();
            move |_| ListView { state }
        });
        let viewport = layout(app, window_id, &state);
        assert!(viewport.at_bottom);
        assert_eq!((viewport.first, viewport.last), (14, 19));

        // A new row at the bottom stays in view.
        state.set_rows(keys(100..121));
        let viewport = layout(app, window_id, &state);
        assert!(viewport.at_bottom);
        assert_eq!(viewport.last, 20);

        // Scrolled up by two and a half rows.
        state.0.borrow_mut().scroll_by(2.5 * ROW);
        let viewport = layout(app, window_id, &state);
        assert!(!viewport.at_bottom);
        assert_eq!(viewport.first, 12);
        let before = state.anchor();
        assert_eq!(
            before,
            Anchor::Row {
                key: key(112),
                offset: -ROW / 2.
            }
        );

        // Older rows loaded above: the same rows stay on screen.
        state.set_rows(keys(90..121));
        let viewport = layout(app, window_id, &state);
        assert_eq!(viewport.first, 22);
        assert_eq!(state.anchor(), before);

        // Newer rows below don't move it either.
        state.set_rows(keys(90..125));
        layout(app, window_id, &state);
        assert_eq!(state.anchor(), before);
    });
}

#[test]
fn clamps_at_both_ends() {
    App::test((), |mut app| async move {
        let app = &mut app;
        let (state, _viewports) = TimelineState::new();
        state.set_rows(keys(0..20));
        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, {
            let state = state.clone();
            move |_| ListView { state }
        });
        layout(app, window_id, &state);

        state.0.borrow_mut().scroll_by(10_000.);
        let viewport = layout(app, window_id, &state);
        assert_eq!((viewport.first, viewport.at_bottom), (0, false));
        assert_eq!(
            state.anchor(),
            Anchor::Row {
                key: key(0),
                offset: 0.
            }
        );

        state.0.borrow_mut().scroll_by(-10_000.);
        assert!(layout(app, window_id, &state).at_bottom);

        // A jump to a row near the end settles at the bottom.
        state.scroll_to(key(18), 0.);
        assert!(layout(app, window_id, &state).at_bottom);
        state.scroll_to(key(5), -10.);
        let viewport = layout(app, window_id, &state);
        assert_eq!((viewport.first, viewport.at_bottom), (5, false));
    });
}

#[test]
fn few_rows_sit_at_the_bottom() {
    App::test((), |mut app| async move {
        let app = &mut app;
        let (state, _viewports) = TimelineState::new();
        state.set_rows(keys(0..3));
        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, {
            let state = state.clone();
            move |_| ListView { state }
        });
        let viewport = layout(app, window_id, &state);
        assert_eq!((viewport.first, viewport.last), (0, 2));
        assert!(viewport.at_bottom);
        // Scrolling does nothing.
        state.0.borrow_mut().scroll_by(100.);
        assert!(layout(app, window_id, &state).at_bottom);
        assert_eq!(state.0.borrow().placed[0], (0, VIEWPORT - 3. * ROW));
    });
}
