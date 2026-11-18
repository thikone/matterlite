// SPDX-License-Identifier: AGPL-3.0-only

//! An element that lets its child be as wide as it wants but takes at most the width it is
//! given, clipping the rest: a row of tabs in a narrow panel doesn't hold the panel open.

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warpui::ClipBounds;
use warpui::elements::{
    AfterLayoutContext, AppContext, Element, EventContext, LayoutContext, PaintContext, Point,
    SizeConstraint,
};
use warpui::event::DispatchedEvent;

pub struct ClipWidth {
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl ClipWidth {
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            size: None,
            origin: None,
        }
    }

    pub fn finish(self) -> Box<dyn Element> {
        Box::new(self)
    }
}

impl Element for ClipWidth {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // The child at its natural width; this element no wider than allowed.
        let unbounded = SizeConstraint::new(
            vec2f(0., constraint.min.y()),
            vec2f(f32::INFINITY, constraint.max.y()),
        );
        let child = self.child.layout(unbounded, ctx, app);
        let width = child.x().min(constraint.max.x()).max(constraint.min.x());
        let size = vec2f(width, child.y());
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self.size.unwrap_or_default();
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        ctx.scene
            .start_layer(ClipBounds::BoundedBy(RectF::new(origin, size)));
        self.child.paint(origin, ctx, app);
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
        self.child.dispatch_event(event, ctx, app)
    }
}
