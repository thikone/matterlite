// SPDX-License-Identifier: AGPL-3.0-only

//! A Markdown table: every column as wide as its widest cell (as the webapp's
//! `markdown__table`, which isn't stretched to the message's width), all columns narrowed in
//! proportion (their text wrapping) when the table doesn't fit, and every cell of a row as
//! tall as the row, so the borders line up.

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpui::color::ColorU;
use warpui::elements::{
    AfterLayoutContext, AppContext, Border, Element, EventContext, LayoutContext, PaintContext,
    Point, SizeConstraint,
};
use warpui::event::DispatchedEvent;

use mm_render::Alignment;

/// Space between a cell's border and its content.
const PADDING_X: f32 = 8.;
const PADDING_Y: f32 = 4.;
/// A column is never narrowed below this (content width).
const MIN_COLUMN: f32 = 24.;

pub struct TableCell {
    pub content: Box<dyn Element>,
    pub alignment: Alignment,
}

pub struct MarkdownTable {
    rows: Vec<Vec<TableCell>>,
    border: ColorU,
    /// Laid out: column widths (content), row heights (content), each cell's content size.
    columns: Vec<f32>,
    heights: Vec<f32>,
    sizes: Vec<Vec<Vector2F>>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl MarkdownTable {
    /// Rows of cells, the header first; rows shorter than the widest get empty cells' space.
    pub fn new(rows: Vec<Vec<TableCell>>, border: ColorU) -> Self {
        Self {
            rows,
            border,
            columns: Vec::new(),
            heights: Vec::new(),
            sizes: Vec::new(),
            size: None,
            origin: None,
        }
    }

    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }
}

/// Column widths (content) from their natural widths: as they are when they fit in `available`
/// (content width of the whole table), else narrowed in proportion, none below `MIN_COLUMN`.
pub fn column_widths(natural: &[f32], available: f32) -> Vec<f32> {
    let total: f32 = natural.iter().sum();
    if total <= available || total <= 0. {
        return natural.to_vec();
    }
    let scale = available.max(0.) / total;
    natural
        .iter()
        .map(|width| (width * scale).max(MIN_COLUMN.min(*width)))
        .collect()
}

impl Element for MarkdownTable {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let count = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        // Natural widths: each cell on one line.
        let mut natural = vec![0f32; count];
        let unbounded = SizeConstraint::new(Vector2F::zero(), vec2f(f32::INFINITY, f32::INFINITY));
        for row in &mut self.rows {
            for (column, cell) in row.iter_mut().enumerate() {
                let size = cell.content.layout(unbounded, ctx, app);
                natural[column] = natural[column].max(size.x().ceil());
            }
        }
        let chrome = count as f32 * (2. * PADDING_X + 1.) + 1.;
        self.columns = column_widths(&natural, constraint.max.x() - chrome);
        // Each cell in its column's width; the row as tall as its tallest cell.
        self.heights.clear();
        self.sizes.clear();
        for row in &mut self.rows {
            let mut height = 0f32;
            let mut sizes = Vec::with_capacity(row.len());
            for (column, cell) in row.iter_mut().enumerate() {
                let width = self.columns[column];
                let size = cell.content.layout(
                    SizeConstraint::new(Vector2F::zero(), vec2f(width, f32::INFINITY)),
                    ctx,
                    app,
                );
                height = height.max(size.y());
                sizes.push(size);
            }
            self.heights.push(height);
            self.sizes.push(sizes);
        }
        let width = self.columns.iter().sum::<f32>() + chrome;
        let height = self.heights.iter().sum::<f32>()
            + self.heights.len() as f32 * (2. * PADDING_Y + 1.)
            + 1.;
        let size = vec2f(width.min(constraint.max.x()), height);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        for row in &mut self.rows {
            for cell in row {
                cell.content.after_layout(ctx, app);
            }
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let mut y = origin.y();
        for (row_index, row) in self.rows.iter_mut().enumerate() {
            let row_height = self.heights[row_index] + 2. * PADDING_Y;
            let mut x = origin.x();
            for (column, width) in self.columns.iter().enumerate() {
                let cell_width = width + 2. * PADDING_X;
                // Neighbouring borders overlap by a pixel: single lines between cells.
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(x, y),
                        vec2f(cell_width + 1., row_height + 1.),
                    ))
                    .with_border(Border::all(1.).with_border_fill(self.border));
                if let (Some(cell), Some(size)) =
                    (row.get_mut(column), self.sizes[row_index].get(column))
                {
                    let free = (width - size.x()).max(0.);
                    let offset = match cell.alignment {
                        Alignment::Center => free / 2.,
                        Alignment::Right => free,
                        Alignment::Left | Alignment::None => 0.,
                    };
                    cell.content.paint(
                        vec2f(x + 1. + PADDING_X + offset, y + 1. + PADDING_Y),
                        ctx,
                        app,
                    );
                }
                x += cell_width + 1.;
            }
            y += row_height + 1.;
        }
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
        for row in &mut self.rows {
            for cell in row {
                handled |= cell.content.dispatch_event(event, ctx, app);
            }
        }
        handled
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
