// SPDX-License-Identifier: AGPL-3.0-only

//! The post toolbar, shown at a post's top right while the pointer is over it, and the post
//! menu it opens (More, or a right-click on the post): the webapp's post hover menu and dot
//! menu (`components/post/post_options.tsx`, `components/dot_menu/dot_menu.tsx`). The entries
//! follow [`PostActions`]; the view showing the posts handles what they do.

use std::rc::Rc;

use mm_model::{Post, PostId};
use mm_store::PostActions;
use pathfinder_geometry::vector::{Vector2F, vec2f};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Dismiss, Flex, Hoverable,
    MouseStateHandle, ParentElement, Radius, SavePosition, Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent as _;
use warpui::{Action, Element, EventContext};

use ml_workspace::icon_button_with_color;

const MENU_WIDTH: f32 = 220.;

/// What the toolbar and the menu dispatch, wrapped in the hosting view's action.
#[derive(Clone, Debug, PartialEq)]
pub enum PostAction {
    /// Opens the menu of a post, under the toolbar's More button (`at` = `None`) or at a
    /// point of the host (a right-click), relative to its [`PostMenuHost::root_position`].
    OpenMenu {
        post: PostId,
        at: Option<Vector2F>,
        /// Opens upwards (the post is in the lower half of the host).
        up: bool,
    },
    CloseMenu,
    Choose(PostMenuItem),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostMenuItem {
    Follow,
    MarkUnread,
    Save,
    Pin,
    CopyLink,
    CopyText,
    Edit,
    Delete,
}

/// Where a view shows posts with actions: names its saved positions and wraps
/// [`PostAction`]s in its own action type.
pub struct PostMenuHost<A: Action + Clone> {
    /// A prefix for the saved positions, unique per view.
    pub prefix: &'static str,
    pub wrap: fn(PostAction) -> A,
}

// Not derived: that would ask `A: Copy`.
impl<A: Action + Clone> Clone for PostMenuHost<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Action + Clone> Copy for PostMenuHost<A> {}

impl<A: Action + Clone> PostMenuHost<A> {
    /// The saved position of the host's whole area (right-click points are relative to it).
    pub fn root_position(&self) -> String {
        format!("{}:posts", self.prefix)
    }

    /// The action of a right-click at `position` (window coordinates) on a post.
    pub fn open_at(&self, ctx: &EventContext, post: PostId, position: Vector2F) -> A {
        let root = ctx.element_position_by_id(self.root_position());
        let origin = root.map(|rect| rect.origin()).unwrap_or_default();
        (self.wrap)(PostAction::OpenMenu {
            post,
            at: Some(position - origin),
            up: root.is_some_and(|rect| position.y() > rect.center().y()),
        })
    }

    /// The action of the toolbar's More button.
    fn open_below_more(&self, ctx: &EventContext, post: PostId) -> A {
        let root = ctx.element_position_by_id(self.root_position());
        let more = ctx.element_position_by_id(self.more_position());
        let up = match (root, more) {
            (Some(root), Some(more)) => more.center().y() > root.center().y(),
            _ => false,
        };
        (self.wrap)(PostAction::OpenMenu { post, at: None, up })
    }

    fn more_position(&self) -> String {
        format!("{}:post_more", self.prefix)
    }
}

/// The open post menu.
pub struct PostMenu {
    pub post: PostId,
    pub at: Option<Vector2F>,
    pub up: bool,
    pub actions: PostActions,
    /// Replies of the post's thread ("Follow thread" or "Follow message").
    pub replies: i64,
    mouse_states: Vec<MouseStateHandle>,
}

impl PostMenu {
    pub fn new(
        post: &Post,
        at: Option<Vector2F>,
        up: bool,
        actions: PostActions,
        replies: i64,
    ) -> Self {
        let mut menu = Self {
            post: post.id.clone(),
            at,
            up,
            actions,
            replies,
            mouse_states: Vec::new(),
        };
        menu.mouse_states = menu
            .items()
            .iter()
            .flatten()
            .map(|_| MouseStateHandle::default())
            .collect();
        menu
    }

    /// The entries in groups, as the webapp orders them, with their labels.
    pub fn items(&self) -> Vec<Vec<(PostMenuItem, &'static str)>> {
        let a = &self.actions;
        let mut first = Vec::new();
        if a.follow {
            let label = match (a.following, self.replies > 0) {
                (true, true) => "Unfollow thread",
                (true, false) => "Unfollow message",
                (false, true) => "Follow thread",
                (false, false) => "Follow message",
            };
            first.push((PostMenuItem::Follow, label));
        }
        if a.mark_unread {
            first.push((PostMenuItem::MarkUnread, "Mark as unread"));
        }
        if a.save {
            let label = if a.saved { "Remove from saved" } else { "Save" };
            first.push((PostMenuItem::Save, label));
        }
        if a.pin {
            let label = if a.pinned {
                "Unpin from channel"
            } else {
                "Pin to channel"
            };
            first.push((PostMenuItem::Pin, label));
        }
        let mut copy = Vec::new();
        if a.copy {
            copy.push((PostMenuItem::CopyLink, "Copy link"));
            copy.push((PostMenuItem::CopyText, "Copy text"));
        }
        let mut change = Vec::new();
        if a.edit {
            change.push((PostMenuItem::Edit, "Edit"));
        }
        if a.delete {
            change.push((PostMenuItem::Delete, "Delete"));
        }
        [first, copy, change]
            .into_iter()
            .filter(|group| !group.is_empty())
            .collect()
    }

    /// The menu, to add to the host's stack with [`Self::positioning`].
    pub fn render<A: Action + Clone>(
        &self,
        host: PostMenuHost<A>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let background = theme.surface_1();
        let text = theme.main_text_color(background).into_solid();
        let danger = theme.chat().error_text;
        let hover = theme.surface_2();
        let line = theme.surface_3();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        let mut states = self.mouse_states.iter();
        for (index, group) in self.items().into_iter().enumerate() {
            if index > 0 {
                column.add_child(
                    Container::new(
                        ConstrainedBox::new(warpui::elements::Empty::new().finish())
                            .with_height(1.)
                            .finish(),
                    )
                    .with_background(line)
                    .with_vertical_margin(4.)
                    .finish(),
                );
            }
            for (item, label) in group {
                let Some(state) = states.next() else { break };
                let color = if item == PostMenuItem::Delete {
                    danger
                } else {
                    text
                };
                let wrap = host.wrap;
                column.add_child(
                    Hoverable::new(state.clone(), move |state| {
                        let mut container = Container::new(
                            Text::new_inline(label, font, size)
                                .with_color(color)
                                .finish(),
                        )
                        .with_horizontal_padding(12.)
                        .with_vertical_padding(6.);
                        if state.is_hovered() {
                            container = container.with_background(hover);
                        }
                        container.finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(wrap(PostAction::Choose(item)))
                    })
                    .finish(),
                );
            }
        }
        let menu = Container::new(
            ConstrainedBox::new(column.finish())
                .with_width(MENU_WIDTH)
                .finish(),
        )
        .with_vertical_padding(4.)
        .with_background(background)
        .with_border(Border::all(1.).with_border_fill(line))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish();
        let wrap = host.wrap;
        Dismiss::new(menu)
            .on_dismiss(move |ctx, _| ctx.dispatch_typed_action(wrap(PostAction::CloseMenu)))
            .finish()
    }

    /// Under (or over) the More button, its right edges aligned; or at the right-click
    /// point.
    pub fn positioning<A: Action + Clone>(
        &self,
        host: PostMenuHost<A>,
    ) -> warpui::elements::OffsetPositioning {
        use warpui::elements::{
            ChildAnchor, OffsetPositioning, PositionedElementAnchor, PositionedElementOffsetBounds,
        };
        let bounds = PositionedElementOffsetBounds::WindowByPosition;
        match (self.at, self.up) {
            (Some(at), up) => OffsetPositioning::offset_from_save_position_element(
                host.root_position(),
                at,
                bounds,
                PositionedElementAnchor::TopLeft,
                if up {
                    ChildAnchor::BottomLeft
                } else {
                    ChildAnchor::TopLeft
                },
            ),
            (None, false) => OffsetPositioning::offset_from_save_position_element(
                host.more_position(),
                vec2f(0., 4.),
                bounds,
                PositionedElementAnchor::BottomRight,
                ChildAnchor::TopRight,
            ),
            (None, true) => OffsetPositioning::offset_from_save_position_element(
                host.more_position(),
                vec2f(0., -4.),
                bounds,
                PositionedElementAnchor::TopRight,
                ChildAnchor::BottomRight,
            ),
        }
    }
}

/// The toolbar's buttons' hover states; one set per view (one post is hovered at a time).
#[derive(Default)]
pub struct ToolbarStates {
    more: MouseStateHandle,
}

/// The toolbar of a post: icon buttons in a small card.
pub fn render_toolbar<A: Action + Clone>(
    host: PostMenuHost<A>,
    states: &Rc<ToolbarStates>,
    post: &PostId,
    menu_open: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let color = theme.sub_text_color(theme.surface_1());
    let tooltip = |label: &str| {
        let tooltip = appearance
            .ui_builder()
            .tool_tip(label.to_owned())
            .build()
            .finish();
        move || tooltip
    };
    let post = post.clone();
    let more = icon_button_with_color(
        appearance,
        Icon::DotsHorizontal,
        menu_open,
        states.more.clone(),
        color,
    )
    .with_tooltip(tooltip("More"))
    .build()
    .on_click(move |ctx, _, _| {
        let action = host.open_below_more(ctx, post.clone());
        ctx.dispatch_typed_action(action)
    })
    .with_cursor(Cursor::PointingHand)
    .finish();
    let row = Flex::row()
        .with_spacing(2.)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(SavePosition::new(more, &host.more_position()).finish())
        .finish();
    Container::new(row)
        .with_uniform_padding(2.)
        .with_background(theme.surface_1())
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish()
}

#[cfg(test)]
#[path = "post_menu_tests.rs"]
mod tests;
