// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/core/window.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use core::fmt;
use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::core::view::AnyViewHandle;
use crate::core::{AnyView, BlurContext, FocusContext};
use crate::{AccessibilityData, AppContext, CursorInfo, EntityId, EntityIdMap, keymap};

/// A unique identifier for a window.
///
/// These are globally unique and not reused across the lifetime of the
/// application.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(usize);

impl WindowId {
    /// Constructs a new globally-unique window ID.
    #[allow(clippy::new_without_default)]
    pub fn new() -> WindowId {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        WindowId(raw)
    }

    pub fn from_usize(value: usize) -> WindowId {
        WindowId(value)
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// A structure holding all application state that is linked to a particular
/// window.
#[derive(Default)]
pub(super) struct Window {
    /// The set of views owned by this window, keyed by view ID.
    pub views: EntityIdMap<StoredView>,

    /// A handle to the window's root view (top of the view hierarchy), if any.
    pub root_view: Option<AnyViewHandle>,

    /// The ID of the currently focused view, if any.
    pub focused_view: Option<EntityId>,
}

/// A type-erased view stored in a window's view registry. The inherent methods delegate to the
/// view; downcasting goes through [`StoredView::as_any`], never the wrapper itself.
pub(crate) struct StoredView(Box<dyn AnyView>);

impl StoredView {
    pub fn new(view: Box<dyn AnyView>) -> Self {
        Self(view)
    }

    pub fn render(&self, app: &AppContext) -> Box<dyn crate::Element> {
        self.0.render(app)
    }

    pub fn as_any(&self) -> &dyn Any {
        self.0.as_any()
    }

    pub fn as_any_mut(&mut self) -> &mut dyn Any {
        self.0.as_any_mut()
    }

    pub fn ui_name(&self) -> &'static str {
        self.0.ui_name()
    }

    pub fn on_focus(
        &mut self,
        focus_ctx: &FocusContext,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) {
        self.0.on_focus(focus_ctx, app, window_id, view_id)
    }

    pub fn on_blur(
        &mut self,
        blur_ctx: &BlurContext,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) {
        self.0.on_blur(blur_ctx, app, window_id, view_id)
    }

    pub fn keymap_context(&self, app: &AppContext) -> keymap::Context {
        self.0.keymap_context(app)
    }

    pub fn active_cursor_position(
        &self,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Option<CursorInfo> {
        self.0.active_cursor_position(app, window_id, view_id)
    }

    pub fn on_window_closed(
        &mut self,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) {
        self.0.on_window_closed(app, window_id, view_id)
    }

    pub fn on_window_transferred(
        &mut self,
        source_window_id: WindowId,
        target_window_id: WindowId,
        app: &mut AppContext,
        view_id: EntityId,
    ) {
        self.0
            .on_window_transferred(source_window_id, target_window_id, app, view_id)
    }

    pub fn self_or_child_interacted_with(
        &self,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) {
        self.0
            .self_or_child_interacted_with(app, window_id, view_id)
    }

    pub fn accessibility_data(
        &self,
        app: &mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Option<AccessibilityData> {
        self.0.accessibility_data(app, window_id, view_id)
    }

    pub fn child_view_ids(&self, app: &AppContext) -> Vec<EntityId> {
        self.0.child_view_ids(app)
    }
}
