// SPDX-License-Identifier: AGPL-3.0-only
// Viewporting modelled on warp-terminal `crates/warpui_core/src/elements/gui/viewported_list.rs`
// @ 5bb7bcc6a (lay out only the visible items, remember measured heights, estimate the rest
// for the scrollbar); the code is new, since a chat needs the list anchored at the bottom and
// rows added above the visible ones without a jump.

//! A list of rows of different heights for the chat timeline. Only visible rows are laid
//! out. The scroll position is an [`Anchor`]: the bottom (new rows stay in view) or a row's
//! key and the offset of its top from the viewport's top. Older rows loaded above, newer rows
//! below and rows changing height elsewhere leave what is on screen where it is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpui::ClipBounds;
use warpui::elements::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext, Point,
    ScrollData, ScrollableElement, SizeConstraint,
};
use warpui::event::DispatchedEvent;
use warpui::units::{IntoPixels as _, Pixels};

use crate::timeline::RowKey;

/// The height assumed for a row not measured yet (a one-line post).
const ESTIMATED_HEIGHT: f32 = 44.;

#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// The newest row's bottom at the viewport's bottom.
    Bottom,
    /// The row's top `offset` pixels below the viewport's top (negative: it starts above).
    Row { key: RowKey, offset: f32 },
}

/// What is on screen, reported after each layout that changed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    /// The first and last visible rows.
    pub first: usize,
    pub last: usize,
    pub rows: usize,
    /// At the bottom, following new rows.
    pub at_bottom: bool,
}

struct Inner {
    /// Keys and content versions, oldest first.
    rows: Vec<(RowKey, u64)>,
    index: HashMap<RowKey, usize>,
    /// Measured heights by key: (version, width, height).
    heights: HashMap<RowKey, (u64, f32, f32)>,
    anchor: Anchor,
    /// Scrolling since the last layout, applied by the next one.
    scrolled: f32,
    /// Where the last layout put the visible rows: (index, top).
    placed: Vec<(usize, f32)>,
    viewport_height: f32,
    reported: Option<Viewport>,
    viewport_tx: async_channel::Sender<Viewport>,
}

/// The list's state, shared between renders (one per chat, so each keeps its position).
#[derive(Clone)]
pub struct TimelineState(Rc<RefCell<Inner>>);

impl TimelineState {
    /// A state anchored at the bottom, and the receiver of its [`Viewport`] reports.
    pub fn new() -> (Self, async_channel::Receiver<Viewport>) {
        let (viewport_tx, viewport_rx) = async_channel::unbounded();
        let state = Self(Rc::new(RefCell::new(Inner {
            rows: Vec::new(),
            index: HashMap::new(),
            heights: HashMap::new(),
            anchor: Anchor::Bottom,
            scrolled: 0.,
            placed: Vec::new(),
            viewport_height: 0.,
            reported: None,
            viewport_tx,
        })));
        (state, viewport_rx)
    }

    /// The rows to show, oldest first, each with a version that changes when its content
    /// (and so maybe its height) does. A row whose key and version stay keeps its height.
    pub fn set_rows(&self, rows: Vec<(RowKey, u64)>) {
        let mut inner = self.0.borrow_mut();
        inner.index = rows
            .iter()
            .enumerate()
            .map(|(index, (key, _))| (key.clone(), index))
            .collect();
        if let Anchor::Row { key, .. } = &inner.anchor
            && !inner.index.contains_key(key)
        {
            // The anchor row went away (a pending post confirmed, the line removed): keep the
            // row that took its place in view.
            let replacement = inner
                .placed
                .first()
                .and_then(|(index, top)| rows.get(*index).map(|(key, _)| (key.clone(), *top)));
            inner.anchor = match replacement {
                Some((key, offset)) => Anchor::Row { key, offset },
                None => Anchor::Bottom,
            };
        }
        let keep: std::collections::HashSet<&RowKey> = rows.iter().map(|(key, _)| key).collect();
        let stale: Vec<RowKey> = inner
            .heights
            .keys()
            .filter(|key| !keep.contains(key))
            .cloned()
            .collect();
        for key in stale {
            inner.heights.remove(&key);
        }
        inner.rows = rows;
        inner.placed.clear();
    }

    /// Forgets the measured heights (theme, font or zoom changed).
    pub fn clear_heights(&self) {
        self.0.borrow_mut().heights.clear();
    }

    #[cfg(test)]
    pub fn anchor(&self) -> Anchor {
        self.0.borrow().anchor.clone()
    }

    pub fn scroll_to_bottom(&self) {
        let mut inner = self.0.borrow_mut();
        inner.anchor = Anchor::Bottom;
        inner.scrolled = 0.;
    }

    /// Shows the row with its top `offset` pixels below the viewport's top (clamped to the
    /// ends of the list).
    pub fn scroll_to(&self, key: RowKey, offset: f32) {
        let mut inner = self.0.borrow_mut();
        if inner.index.contains_key(&key) {
            inner.anchor = Anchor::Row { key, offset };
            inner.scrolled = 0.;
        }
    }

    /// Scrolls by a page (`up`: towards older rows).
    pub fn scroll_page(&self, up: bool) {
        let mut inner = self.0.borrow_mut();
        let page = (inner.viewport_height - 40.).max(40.);
        inner.scroll_by(if up { page } else { -page });
    }
}

impl Inner {
    /// `delta` > 0 moves the content down (shows older rows), as the wheel reports it.
    fn scroll_by(&mut self, delta: f32) {
        if self.anchor == Anchor::Bottom {
            if delta <= 0. {
                return;
            }
            // Leave the bottom from where the rows are now.
            if let Some((index, top)) = self.placed.first().copied()
                && let Some((key, _)) = self.rows.get(index)
            {
                self.anchor = Anchor::Row {
                    key: key.clone(),
                    offset: top,
                };
            } else {
                return;
            }
        }
        self.scrolled += delta;
    }

    fn height(&self, index: usize, width: f32) -> Option<f32> {
        let (key, version) = &self.rows[index];
        self.heights
            .get(key)
            .filter(|(measured_version, measured_width, _)| {
                measured_version == version && (measured_width - width).abs() < 0.5
            })
            .map(|(_, _, height)| *height)
    }

    fn total_height(&self, width: f32) -> f32 {
        (0..self.rows.len())
            .map(|index| self.height(index, width).unwrap_or(ESTIMATED_HEIGHT))
            .sum()
    }

    fn height_above(&self, index: usize, width: f32) -> f32 {
        (0..index)
            .map(|index| self.height(index, width).unwrap_or(ESTIMATED_HEIGHT))
            .sum()
    }
}

type RenderRow = dyn Fn(usize, &AppContext) -> Box<dyn Element>;

/// The element. `render_row(index)` builds row `index` of the rows given to the state.
pub struct TimelineList {
    state: TimelineState,
    render_row: Box<RenderRow>,
    children: Vec<(f32, Box<dyn Element>)>,
    size: Option<Vector2F>,
    origin: Option<Point>,
    scroll: ScrollData,
}

impl TimelineList {
    pub fn new(
        state: TimelineState,
        render_row: impl Fn(usize, &AppContext) -> Box<dyn Element> + 'static,
    ) -> Self {
        Self {
            state,
            render_row: Box::new(render_row),
            children: Vec::new(),
            size: None,
            origin: None,
            scroll: ScrollData {
                scroll_start: Pixels::zero(),
                visible_px: Pixels::zero(),
                total_size: Pixels::zero(),
            },
        }
    }
}

impl Element for TimelineList {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = constraint.max;
        let (width, height) = (size.x(), size.y());
        let row_constraint = SizeConstraint::new(vec2f(width, 0.), vec2f(width, f32::INFINITY));
        let mut inner = self.state.0.borrow_mut();
        inner.viewport_height = height;
        let count = inner.rows.len();
        self.children.clear();
        self.size = Some(size);
        if count == 0 {
            inner.placed.clear();
            self.scroll = ScrollData {
                scroll_start: Pixels::zero(),
                visible_px: height.into_pixels(),
                total_size: Pixels::zero(),
            };
            return size;
        }

        // Rows laid out this pass, by index.
        let mut laid_out: HashMap<usize, (Box<dyn Element>, f32)> = HashMap::new();
        let mut measure = |index: usize, inner: &mut Inner| -> f32 {
            if let Some((_, row_height)) = laid_out.get(&index) {
                return *row_height;
            }
            let mut element = (self.render_row)(index, app);
            let row_height = element.layout(row_constraint, ctx, app).y();
            let (key, version) = inner.rows[index].clone();
            inner.heights.insert(key, (version, width, row_height));
            laid_out.insert(index, (element, row_height));
            row_height
        };

        // Where the anchor row goes.
        let scrolled = std::mem::take(&mut inner.scrolled);
        let start = match inner.anchor.clone() {
            Anchor::Row { key, offset } => inner
                .index
                .get(&key)
                .map(|index| (*index, offset + scrolled)),
            Anchor::Bottom => None,
        };
        let mut placed: Vec<(usize, f32)> = Vec::new();
        let mut at_bottom = start.is_none();
        if let Some((anchor, offset)) = start {
            // Down from the anchor, then up from it.
            let mut index = anchor;
            let mut top = offset;
            // Scrolled up past rows: start from the first one still visible.
            while top > 0. && index > 0 {
                index -= 1;
                top -= measure(index, &mut inner);
            }
            if index == 0 && top > 0. {
                // Scrolled past the oldest row.
                top = 0.;
            }
            while index + 1 < count && top + measure(index, &mut inner) <= 0. {
                top += measure(index, &mut inner);
                index += 1;
            }
            let first = (index, top);
            let mut y = top;
            while index < count && y < height {
                placed.push((index, y));
                y += measure(index, &mut inner);
                index += 1;
            }
            if index == count && y <= height {
                // The newest row is in view with room below it: back at the bottom.
                at_bottom = true;
                placed.clear();
            } else {
                // Fill above the first row.
                let (mut index, mut y) = first;
                while y > 0. && index > 0 {
                    index -= 1;
                    y -= measure(index, &mut inner);
                    placed.insert(0, (index, y));
                }
                // Scrolled past the oldest row: pin it to the top.
                if let Some(&(0, top)) = placed.first()
                    && top > 0.
                {
                    for (_, y) in &mut placed {
                        *y -= top;
                    }
                    // Rows that moved into view at the bottom.
                    let (mut index, mut y) = placed
                        .last()
                        .map(|(index, y)| (*index, *y))
                        .unwrap_or((0, 0.));
                    y += measure(index, &mut inner);
                    index += 1;
                    while index < count && y < height {
                        placed.push((index, y));
                        y += measure(index, &mut inner);
                        index += 1;
                    }
                    if index == count && y <= height {
                        at_bottom = true;
                        placed.clear();
                    }
                }
            }
        }
        if at_bottom {
            // Up from the newest row.
            let mut y = height;
            let mut index = count;
            while index > 0 && y > 0. {
                index -= 1;
                y -= measure(index, &mut inner);
                placed.insert(0, (index, y));
            }
            inner.anchor = Anchor::Bottom;
        } else if let Some(&(index, top)) = placed
            .iter()
            .find(|(index, _)| inner.rows[*index].0.holds_place())
            .or(placed.first())
        {
            // Anchor at the first visible post, so rows loaded above it don't move it.
            inner.anchor = Anchor::Row {
                key: inner.rows[index].0.clone(),
                offset: top,
            };
        }

        for (index, top) in &placed {
            if let Some((element, _)) = laid_out.remove(index) {
                self.children.push((*top, element));
            }
        }
        let total = inner.total_height(width).max(height);
        let first_top = placed
            .first()
            .map_or(0., |(index, top)| inner.height_above(*index, width) - top);
        // Bottom-aligned content shorter than the viewport starts at 0.
        let scroll_start = if at_bottom {
            (total - height).max(0.)
        } else {
            first_top.clamp(0., (total - height).max(0.))
        };
        self.scroll = ScrollData {
            scroll_start: scroll_start.into_pixels(),
            visible_px: height.into_pixels(),
            total_size: total.into_pixels(),
        };
        let viewport = Viewport {
            first: placed.first().map_or(0, |(index, _)| *index),
            last: placed.last().map_or(0, |(index, _)| *index),
            rows: count,
            at_bottom,
        };
        inner.placed = placed;
        if inner.reported != Some(viewport) {
            inner.reported = Some(viewport);
            let _ = inner.viewport_tx.try_send(viewport);
        }
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        for (_, child) in &mut self.children {
            child.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self.size.unwrap_or_default();
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        ctx.scene
            .start_layer(ClipBounds::BoundedBy(RectF::new(origin, size)));
        for (top, child) in &mut self.children {
            child.paint(origin + vec2f(0., *top), ctx, app);
        }
        ctx.scene.stop_layer();
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let mut handled = false;
        for (_, child) in &mut self.children {
            handled |= child.dispatch_event(event, ctx, app);
        }
        handled
    }
}

impl ScrollableElement for TimelineList {
    fn scroll_data(&self, _: &AppContext) -> Option<ScrollData> {
        Some(self.scroll)
    }

    fn scroll(&mut self, delta: Pixels, ctx: &mut EventContext) {
        self.state.0.borrow_mut().scroll_by(delta.as_f32());
        ctx.notify();
    }

    fn should_handle_scroll_wheel(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "timeline_list_tests.rs"]
mod tests;
