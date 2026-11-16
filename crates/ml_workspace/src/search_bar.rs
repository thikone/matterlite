// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/workspace/view/session_history.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! The search bar shared by the History, Files and Search panes and the Chats and Activity
//! filters (MatterLite.md §10.10).
//!
//! A bordered field (magnifier, single-line editor, Clear, Regex and Case-sensitive toggles)
//! above an optional bar of icon toggles in groups. The bar owns the query and the toggle
//! state; the pane owns the matching (local and server-side) and builds its matcher with
//! [`SearchBarView::matcher`].
//!
//! Keyboard: Up/Down/PageUp/PageDown/Tab arrive as [`SearchBarEvent::Navigate`] so the pane can
//! move through its results while the field keeps focus; Enter emits
//! [`SearchBarEvent::Submit`]; Esc clears the query, and on an empty query emits
//! [`SearchBarEvent::Dismissed`] so the pane can move focus away.

use std::borrow::Cow;

use ml_editor::{
    EditorView, Event as EditorEvent, NavigationKey, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use regex::{Regex, RegexBuilder};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill as ThemeFill;
use warpui::elements::{
    Border, ChildAnchor, ChildView, Clipped, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, DispatchEventResult, EventHandler, Flex, Hoverable, MouseStateHandle,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Rect, Shrinkable,
    Stack,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent as _;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

/// Matches Warp's find bar and block filter.
const FIELD_FONT_SIZE: f32 = 12.;
const FIELD_ICON_SIZE: f32 = 12.;
const FIELD_PADDING: f32 = 6.;
const FIELD_BORDER_WIDTH: f32 = 1.;
const FIELD_BORDER_RADIUS: f32 = 6.;
const TOGGLE_ICON_SIZE: f32 = 14.;
const TOGGLE_ICON_PADDING: f32 = 4.;
const TOGGLE_ICON_SPACING: f32 = 4.;
const TOGGLE_CORNER_RADIUS: f32 = 4.;

const CLEAR_TOOLTIP: &str = "Clear search";
const REGEX_TOOLTIP: &str = "Regular expression";
const CASE_SENSITIVE_TOOLTIP: &str = "Case sensitive";

/// One icon toggle in the toggle bar.
#[derive(Clone, Debug)]
pub struct ToggleSpec {
    /// Reported in [`SearchBarEvent::ToggleChanged`] and accepted by
    /// [`SearchBarView::is_toggle_on`].
    pub id: &'static str,
    pub icon: Icon,
    pub tooltip: &'static str,
    /// Initial state.
    pub on: bool,
}

/// Toggles drawn together; groups are separated by a 1-px rule.
#[derive(Clone, Debug, Default)]
pub struct ToggleGroup {
    pub toggles: Vec<ToggleSpec>,
    /// Refuse to turn off the last toggle that is on, as for scope toggles (a query must
    /// always search something).
    pub keep_one_on: bool,
}

#[derive(Clone, Debug)]
pub struct SearchBarOptions {
    pub placeholder: Cow<'static, str>,
    /// Show the Regex and Case-sensitive toggles. Off for server-only searches, which are
    /// word-based.
    pub query_options: bool,
    pub toggle_groups: Vec<ToggleGroup>,
}

impl Default for SearchBarOptions {
    fn default() -> Self {
        Self {
            placeholder: Cow::Borrowed("Search"),
            query_options: true,
            toggle_groups: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SearchBarEvent {
    /// The query text, the regex option or the case option changed: rebuild the matcher and
    /// drop anything cached for the previous query.
    QueryChanged,
    ToggleChanged {
        id: &'static str,
        on: bool,
    },
    /// Enter in the field.
    Submit,
    /// The query was cleared with the Clear button or Esc.
    Cleared,
    /// Esc on an empty query.
    Dismissed,
    Navigate(NavigationKey),
}

#[derive(Clone, Copy, Debug)]
pub enum SearchBarAction {
    Toggle { group: usize, index: usize },
    ToggleRegex,
    ToggleCaseSensitive,
    Clear,
    FieldClicked,
}

struct ToggleState {
    spec: ToggleSpec,
    on: bool,
    mouse_state: MouseStateHandle,
}

#[derive(Default)]
struct MouseStates {
    clear: MouseStateHandle,
    regex: MouseStateHandle,
    case_sensitive: MouseStateHandle,
}

pub struct SearchBarView {
    editor: ViewHandle<EditorView>,
    query_options: bool,
    regex: bool,
    case_sensitive: bool,
    groups: Vec<(bool, Vec<ToggleState>)>,
    mouse_states: MouseStates,
}

impl SearchBarView {
    pub fn new(options: SearchBarOptions, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        let editor = {
            let appearance = Appearance::as_ref(ctx);
            let editor_options = SingleLineEditorOptions {
                // `ui_text` sets the family too; overriding only the size would leave the
                // editor on its monospace default.
                text: TextOptions::ui_text(Some(FIELD_FONT_SIZE), appearance),
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            ctx.add_typed_action_view(|ctx| EditorView::single_line(editor_options, ctx))
        };
        let placeholder = options.placeholder.into_owned();
        editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(placeholder, ctx)
        });
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| {
            me.handle_editor_event(event, ctx)
        });

        let groups = options
            .toggle_groups
            .into_iter()
            .map(|group| {
                let toggles = group
                    .toggles
                    .into_iter()
                    .map(|spec| ToggleState {
                        on: spec.on,
                        spec,
                        mouse_state: MouseStateHandle::default(),
                    })
                    .collect();
                (group.keep_one_on, toggles)
            })
            .collect();

        Self {
            editor,
            query_options: options.query_options,
            regex: false,
            case_sensitive: false,
            groups,
            mouse_states: MouseStates::default(),
        }
    }

    pub fn query(&self, ctx: &AppContext) -> String {
        self.editor.as_ref(ctx).buffer_text(ctx)
    }

    pub fn set_query(&mut self, query: &str, ctx: &mut ViewContext<Self>) {
        self.editor
            .update(ctx, |editor, ctx| editor.set_buffer_text(query, ctx));
    }

    /// Clears the query (undoable, like typing) and emits [`SearchBarEvent::Cleared`].
    pub fn clear(&mut self, ctx: &mut ViewContext<Self>) {
        self.editor
            .update(ctx, |editor, ctx| editor.system_clear_buffer(false, ctx));
        ctx.emit(SearchBarEvent::Cleared);
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.editor);
    }

    pub fn is_focused(&self, ctx: &AppContext) -> bool {
        self.editor.as_ref(ctx).is_focused()
    }

    pub fn regex_enabled(&self) -> bool {
        self.query_options && self.regex
    }

    pub fn case_sensitive(&self) -> bool {
        self.query_options && self.case_sensitive
    }

    /// The state of the toggle with this id; `false` for an unknown id.
    pub fn is_toggle_on(&self, id: &str) -> bool {
        self.groups
            .iter()
            .flat_map(|(_, toggles)| toggles)
            .any(|toggle| toggle.spec.id == id && toggle.on)
    }

    /// Ids of the toggles that are on, in bar order.
    pub fn toggles_on(&self) -> Vec<&'static str> {
        self.groups
            .iter()
            .flat_map(|(_, toggles)| toggles)
            .filter(|toggle| toggle.on)
            .map(|toggle| toggle.spec.id)
            .collect()
    }

    /// The matcher for the current query and options. `None` means "don't filter": the query
    /// is empty or is an invalid (possibly half-typed) regex.
    pub fn matcher(&self, ctx: &AppContext) -> Option<Matcher> {
        Matcher::new(
            &self.query(ctx),
            self.regex_enabled(),
            self.case_sensitive(),
        )
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => ctx.emit(SearchBarEvent::QueryChanged),
            EditorEvent::Enter => ctx.emit(SearchBarEvent::Submit),
            EditorEvent::Escape => {
                if self.query(ctx).is_empty() {
                    ctx.emit(SearchBarEvent::Dismissed);
                } else {
                    self.clear(ctx);
                }
            }
            EditorEvent::Navigate(key) => ctx.emit(SearchBarEvent::Navigate(*key)),
            _ => {}
        }
    }

    /// Sets a toggle from outside (e.g. a "Show all" link turning a filter off), without
    /// [`SearchBarEvent::ToggleChanged`].
    pub fn set_toggle(&mut self, id: &str, on: bool, ctx: &mut ViewContext<Self>) {
        for (_, toggles) in &mut self.groups {
            for toggle in toggles.iter_mut().filter(|toggle| toggle.spec.id == id) {
                toggle.on = on;
            }
        }
        ctx.notify();
    }

    fn toggle(&mut self, group: usize, index: usize, ctx: &mut ViewContext<Self>) {
        let Some((keep_one_on, toggles)) = self.groups.get_mut(group) else {
            return;
        };
        let on_count = toggles.iter().filter(|toggle| toggle.on).count();
        let Some(toggle) = toggles.get_mut(index) else {
            return;
        };
        if *keep_one_on && toggle.on && on_count <= 1 {
            return;
        }
        toggle.on = !toggle.on;
        let (id, on) = (toggle.spec.id, toggle.on);
        ctx.emit(SearchBarEvent::ToggleChanged { id, on });
        ctx.notify();
    }

    /// Warp's find-bar toggle: accent border and selection background when on, so the state
    /// reads without hovering.
    fn render_toggle(
        &self,
        icon: Icon,
        active: bool,
        mouse_state: MouseStateHandle,
        tooltip: &'static str,
        action: SearchBarAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Hoverable::new(mouse_state, |state| {
            let theme = appearance.theme();
            let (border, background) = if active {
                (
                    Border::all(1.).with_border_fill(theme.accent()),
                    theme.find_bar_button_selection_color(),
                )
            } else if state.is_hovered() {
                let hover = theme.foreground_button_color();
                (Border::all(1.).with_border_fill(hover), hover)
            } else {
                let transparent = ThemeFill::Solid(ColorU::transparent_black());
                (Border::all(1.).with_border_fill(transparent), transparent)
            };
            let icon_color = if active {
                theme.active_ui_text_color()
            } else {
                theme.nonactive_ui_text_color()
            };

            let button = Container::new(
                ConstrainedBox::new(icon.to_warpui_icon(icon_color).finish())
                    .with_height(TOGGLE_ICON_SIZE)
                    .with_width(TOGGLE_ICON_SIZE)
                    .finish(),
            )
            .with_uniform_padding(TOGGLE_ICON_PADDING)
            .with_margin_right(TOGGLE_ICON_SPACING)
            .with_border(border)
            .with_background(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(TOGGLE_CORNER_RADIUS)))
            .finish();

            let mut stack = Stack::new().with_child(button);
            if state.is_hovered() {
                stack.add_positioned_overlay_child(
                    appearance
                        .ui_builder()
                        .tool_tip(tooltip.to_string())
                        .build()
                        .finish(),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0., -5.),
                        // Not `Unbounded`: a tooltip centred on a toggle at the panel's left
                        // edge would overflow past x=0 and be clipped.
                        ParentOffsetBounds::WindowByPosition,
                        ParentAnchor::TopMiddle,
                        ChildAnchor::BottomMiddle,
                    ),
                );
            }
            stack.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action))
        .finish()
    }

    fn render_group_separator(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            ConstrainedBox::new(
                Rect::new()
                    .with_background(appearance.theme().split_pane_border_color())
                    .finish(),
            )
            .with_width(1.)
            .with_height(TOGGLE_ICON_SIZE)
            .finish(),
        )
        .with_margin_left(TOGGLE_ICON_SPACING)
        .with_margin_right(TOGGLE_ICON_SPACING * 2.)
        .finish()
    }

    /// Magnifier and editor, then Clear and the query options, inside one border and without
    /// a fill: the tool panel shows the window background through everything it doesn't paint.
    fn render_field(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let magnifier = Container::new(
            ConstrainedBox::new(
                Icon::Search
                    .to_warpui_icon(theme.sub_text_color(theme.background()))
                    .finish(),
            )
            .with_height(FIELD_ICON_SIZE)
            .with_width(FIELD_ICON_SIZE)
            .finish(),
        )
        .with_padding_right(FIELD_PADDING)
        .finish();

        // Only the magnifier and the editor focus on click; wrapping the whole row would
        // swallow clicks on the toggles.
        let query = EventHandler::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(magnifier)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Clipped::new(ChildView::new(&self.editor).finish()).finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .on_left_mouse_down(|ctx, _, _| {
            ctx.dispatch_typed_action(SearchBarAction::FieldClicked);
            DispatchEventResult::StopPropagation
        })
        .finish();

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1., query).finish())
            .with_child(self.render_toggle(
                Icon::XCircle,
                false,
                self.mouse_states.clear.clone(),
                CLEAR_TOOLTIP,
                SearchBarAction::Clear,
                appearance,
            ));
        if self.query_options {
            row.add_child(self.render_group_separator(appearance));
            row.add_child(self.render_toggle(
                Icon::Regex,
                self.regex,
                self.mouse_states.regex.clone(),
                REGEX_TOOLTIP,
                SearchBarAction::ToggleRegex,
                appearance,
            ));
            row.add_child(self.render_toggle(
                Icon::CaseSensitivity,
                self.case_sensitive,
                self.mouse_states.case_sensitive.clone(),
                CASE_SENSITIVE_TOOLTIP,
                SearchBarAction::ToggleCaseSensitive,
                appearance,
            ));
        }

        Container::new(row.finish())
            .with_padding_left(FIELD_PADDING)
            .with_vertical_padding(FIELD_PADDING)
            .with_border(Border::all(FIELD_BORDER_WIDTH).with_border_fill(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(FIELD_BORDER_RADIUS)))
            .with_margin_bottom(4.)
            .finish()
    }

    fn render_toggle_bar(&self, appearance: &Appearance) -> Box<dyn Element> {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for (group_index, (_, toggles)) in self.groups.iter().enumerate() {
            if group_index > 0 {
                row.add_child(self.render_group_separator(appearance));
            }
            for (index, toggle) in toggles.iter().enumerate() {
                row.add_child(self.render_toggle(
                    toggle.spec.icon,
                    toggle.on,
                    toggle.mouse_state.clone(),
                    toggle.spec.tooltip,
                    SearchBarAction::Toggle {
                        group: group_index,
                        index,
                    },
                    appearance,
                ));
            }
        }
        Container::new(row.finish())
            .with_padding_bottom(4.)
            .finish()
    }
}

impl Entity for SearchBarView {
    type Event = SearchBarEvent;
}

impl View for SearchBarView {
    fn ui_name() -> &'static str {
        "SearchBarView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_field(appearance));
        if self.groups.iter().any(|(_, toggles)| !toggles.is_empty()) {
            column.add_child(self.render_toggle_bar(appearance));
        }
        column.finish()
    }
}

impl TypedActionView for SearchBarView {
    type Action = SearchBarAction;

    fn handle_action(&mut self, action: &SearchBarAction, ctx: &mut ViewContext<Self>) {
        match *action {
            SearchBarAction::Toggle { group, index } => self.toggle(group, index, ctx),
            SearchBarAction::ToggleRegex => {
                self.regex = !self.regex;
                ctx.emit(SearchBarEvent::QueryChanged);
                ctx.notify();
            }
            SearchBarAction::ToggleCaseSensitive => {
                self.case_sensitive = !self.case_sensitive;
                ctx.emit(SearchBarEvent::QueryChanged);
                ctx.notify();
            }
            SearchBarAction::Clear => self.clear(ctx),
            SearchBarAction::FieldClicked => self.focus(ctx),
        }
    }
}

/// A query matcher: regex, or a literal substring, case-sensitive or not.
#[derive(Clone, Debug)]
pub enum Matcher {
    Regex(Regex),
    Literal(String),
    /// The needle is stored lowercased.
    LiteralInsensitive(String),
}

impl Matcher {
    /// `None` for an empty query or an invalid regex, so a half-typed pattern doesn't blank
    /// the list.
    pub fn new(query: &str, regex: bool, case_sensitive: bool) -> Option<Self> {
        if query.is_empty() {
            None
        } else if regex {
            RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .ok()
                .map(Self::Regex)
        } else if case_sensitive {
            Some(Self::Literal(query.to_string()))
        } else {
            Some(Self::LiteralInsensitive(query.to_lowercase()))
        }
    }

    pub fn is_match(&self, haystack: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(haystack),
            Self::Literal(needle) => haystack.contains(needle.as_str()),
            Self::LiteralInsensitive(needle) => haystack.to_lowercase().contains(needle.as_str()),
        }
    }

    /// Non-overlapping matches in `haystack`.
    pub fn count(&self, haystack: &str) -> usize {
        match self {
            Self::Regex(regex) => regex.find_iter(haystack).count(),
            Self::Literal(needle) => haystack.matches(needle.as_str()).count(),
            Self::LiteralInsensitive(needle) => {
                haystack.to_lowercase().matches(needle.as_str()).count()
            }
        }
    }
}

#[cfg(test)]
#[path = "search_bar_tests.rs"]
mod tests;
