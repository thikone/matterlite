// SPDX-License-Identifier: AGPL-3.0-only

//! The tool panel's Chats tab (MatterLite.md §10.6): the team switcher with the other teams'
//! unread badge, a filter field, and the team's sidebar (categories, channels, DMs and GMs
//! with unread and mention badges and status dots), drawn in the theme's chat colours.

use std::rc::Rc;

use ml_themes::ChatColors;
use ml_workspace::{SearchBarEvent, SearchBarOptions, SearchBarView};
use mm_model::{CategoryId, ChannelId, TeamId};
use mm_store::StoreDelta;
use mm_sync::Command;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::Fill;
use warpui::color::ColorU;
use warpui::elements::{
    Align, Border, ChildAnchor, ChildView, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Dismiss, Empty, Expanded, Flex, Hoverable, MainAxisSize, MouseStateHandle,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, ScrollStateHandle,
    Scrollable, ScrollableElement, Shrinkable, Stack, Text, UniformList, UniformListState,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats::{self, ChannelIcon, ChannelRow, Presence, Row, TeamEntry};
use crate::session::{Session, SessionEvent};

const ROW_HEIGHT: f32 = 28.;
const TEAM_HEADER_HEIGHT: f32 = 40.;
const TEAM_MENU_WIDTH: f32 = 240.;
const ICON_SIZE: f32 = 14.;
const ROW_PADDING: f32 = 16.;
const READ_OPACITY: f32 = 0.64;
const SCROLLBAR_WIDTH: warpui::elements::ScrollbarWidth = warpui::elements::ScrollbarWidth::Auto;

/// UI state keys (saved per account).
const LAST_TEAM: &str = "last_team";
fn last_channel_key(team: &TeamId) -> String {
    format!("last_channel.{team}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatsAction {
    Open(ChannelId),
    ToggleCategory(CategoryId),
    ToggleTeamMenu,
    CloseTeamMenu,
    SelectTeam(TeamId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatsEvent {
    /// A chat was chosen (click, Enter in the filter, Alt+↑/↓).
    Open(ChannelId),
    TeamChanged(Option<TeamId>),
    /// Esc in an empty filter: give the focus back.
    Dismissed,
}

pub struct ChatsView {
    team: Option<TeamId>,
    current: Option<ChannelId>,
    teams: Vec<TeamEntry>,
    rows: Rc<Vec<Row>>,
    filter: ViewHandle<SearchBarView>,
    team_menu_open: bool,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    /// One per row index, created here so hover survives re-renders.
    row_mouse_states: Rc<Vec<MouseStateHandle>>,
    team_mouse_states: Vec<MouseStateHandle>,
    team_button_mouse_state: MouseStateHandle,
}

impl ChatsView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| {
            let relevant = match event {
                SessionEvent::StateChanged | SessionEvent::Reloaded => true,
                SessionEvent::Changed(deltas) => deltas.iter().any(affects_sidebar),
                _ => false,
            };
            if relevant {
                me.refresh(ctx);
            }
        });
        let filter = ctx.add_typed_action_view(|ctx| {
            SearchBarView::new(
                SearchBarOptions {
                    placeholder: "Find chats".into(),
                    query_options: false,
                    toggle_groups: Vec::new(),
                },
                ctx,
            )
        });
        ctx.subscribe_to_view(&filter, |me, _, event, ctx| match event {
            SearchBarEvent::QueryChanged | SearchBarEvent::Cleared => me.refresh(ctx),
            SearchBarEvent::Submit => {
                if let Some(row) = me.rows.iter().find_map(Row::channel) {
                    let id = row.id.clone();
                    me.open(id, ctx);
                }
            }
            SearchBarEvent::Dismissed => ctx.emit(ChatsEvent::Dismissed),
            _ => {}
        });
        let mut view = Self {
            team: None,
            current: None,
            teams: Vec::new(),
            rows: Rc::default(),
            filter,
            team_menu_open: false,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            row_mouse_states: Rc::default(),
            team_mouse_states: Vec::new(),
            team_button_mouse_state: MouseStateHandle::default(),
        };
        view.refresh(ctx);
        view
    }

    pub fn team(&self) -> Option<&TeamId> {
        self.team.as_ref()
    }

    pub fn current(&self) -> Option<&ChannelId> {
        self.current.as_ref()
    }

    /// Opens the chat `delta` rows away (Alt+↑/↓), or the next unread one (Alt+Shift+↑/↓).
    pub fn step(&mut self, delta: isize, unread_only: bool, ctx: &mut ViewContext<Self>) {
        if let Some(channel) = chats::step(&self.rows, self.current.as_ref(), delta, unread_only) {
            self.open(channel, ctx);
        }
    }

    /// Asks to open a chat; the owner opens its tab and calls [`Self::show_channel`].
    pub fn open(&mut self, channel: ChannelId, ctx: &mut ViewContext<Self>) {
        ctx.emit(ChatsEvent::Open(channel));
    }

    /// Marks the chat of the active tab as current, switching to its team if it belongs to
    /// another one (DMs and GMs belong to every team). `None`: no tab is open.
    pub fn show_channel(&mut self, channel: Option<&ChannelId>, ctx: &mut ViewContext<Self>) {
        let team = channel.and_then(|channel| {
            Session::as_ref(ctx)
                .store()?
                .read()
                .ok()?
                .channels
                .get(channel)
                .map(|channel| channel.team_id.clone())
                .filter(|team| !team.is_empty())
        });
        if let Some(team) = team
            && self.team.as_ref() != Some(&team)
        {
            self.team = Some(team.clone());
            Session::handle(ctx).update(ctx, |session, _| {
                session.set_ui_state(LAST_TEAM, Some(team.to_string()))
            });
            ctx.emit(ChatsEvent::TeamChanged(Some(team)));
        }
        self.current = channel.cloned();
        if let (Some(team), Some(channel)) = (self.team.clone(), channel) {
            Session::handle(ctx).update(ctx, |session, _| {
                session.set_ui_state(&last_channel_key(&team), Some(channel.to_string()))
            });
        }
        self.refresh(ctx);
    }

    fn select_team(&mut self, team: TeamId, ctx: &mut ViewContext<Self>) {
        self.team_menu_open = false;
        if self.team.as_ref() == Some(&team) {
            ctx.notify();
            return;
        }
        self.team = Some(team.clone());
        self.current = None;
        let last = Session::handle(ctx).update(ctx, |session, _| {
            session.set_ui_state(LAST_TEAM, Some(team.to_string()));
            session
                .active()
                .and_then(|active| active.ui_state(&last_channel_key(&team)))
                .map(ChannelId::from)
        });
        ctx.emit(ChatsEvent::TeamChanged(Some(team)));
        self.refresh(ctx);
        // As in the webapp, the team's last chat opens again.
        if let Some(channel) = last
            && self
                .rows
                .iter()
                .filter_map(Row::channel)
                .any(|row| row.id == channel)
        {
            self.open(channel, ctx);
        }
    }

    fn toggle_category(&mut self, category: CategoryId, ctx: &mut ViewContext<Self>) {
        let Some(team) = self.team.clone() else {
            return;
        };
        let collapsed = self.rows.iter().find_map(|row| match row {
            Row::Header {
                category: Some(id),
                collapsed,
                ..
            } if *id == category => Some(*collapsed),
            _ => None,
        });
        if let Some(collapsed) = collapsed {
            Session::as_ref(ctx).send(Command::SetCategoryCollapsed {
                team,
                category,
                collapsed: !collapsed,
            });
        }
    }

    /// Re-reads teams and rows from the store.
    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let filter = self.filter.as_ref(ctx).query(ctx);
        let session = Session::as_ref(ctx);
        let saved_team = session
            .active()
            .and_then(|active| active.ui_state(LAST_TEAM))
            .map(TeamId::from);
        let Some(store) = session.store().and_then(|store| store.read().ok()) else {
            self.teams.clear();
            self.rows = Rc::default();
            ctx.notify();
            return;
        };
        self.teams = chats::teams(&store);
        let known = |team: &TeamId| self.teams.iter().any(|entry| &entry.id == team);
        let team = self
            .team
            .clone()
            .filter(known)
            .or_else(|| saved_team.filter(known))
            .or_else(|| self.teams.first().map(|team| team.id.clone()));
        let team_changed = team != self.team;
        self.rows = Rc::new(match &team {
            Some(team) => chats::rows(&store, team, self.current.as_ref(), &filter),
            None => Vec::new(),
        });
        drop(store);
        self.team = team;
        if self.row_mouse_states.len() < self.rows.len() {
            let mut states = (*self.row_mouse_states).clone();
            states.resize_with(self.rows.len(), MouseStateHandle::default);
            self.row_mouse_states = Rc::new(states);
        }
        self.team_mouse_states
            .resize_with(self.teams.len(), MouseStateHandle::default);
        if team_changed {
            ctx.emit(ChatsEvent::TeamChanged(self.team.clone()));
        }
        ctx.notify();
    }

    fn team_name(&self) -> String {
        self.team
            .as_ref()
            .and_then(|team| self.teams.iter().find(|entry| &entry.id == team))
            .map_or_else(|| "No team".to_owned(), |team| team.name.clone())
    }

    fn render_team_header(&self, chat: &ChatColors, appearance: &Appearance) -> Box<dyn Element> {
        let font = appearance.ui_font_family();
        let others = chats::other_teams_unread(&self.teams, self.team.as_ref());
        let name = self.team_name();
        let chat = *chat;
        let several = self.teams.len() > 1;
        let button = Hoverable::new(self.team_button_mouse_state.clone(), move |state| {
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Text::new_inline(name, font, 15.)
                            .with_color(chat.sidebar_header_text)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .finish(),
                );
            if several {
                row.add_child(
                    Container::new(icon(Icon::ChevronDown, chat.sidebar_header_text, 12.))
                        .with_margin_left(6.)
                        .finish(),
                );
            }
            let mut container = Container::new(row.finish())
                .with_horizontal_padding(6.)
                .with_vertical_padding(4.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            if state.is_hovered() && several {
                container = container.with_background_color(fade(chat.sidebar_header_text, 0.12));
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(ChatsAction::ToggleTeamMenu))
        .finish();

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1., button).finish());
        // Activity in the other teams: a mention count, or a dot for unread messages.
        if others.mentions > 0 {
            row.add_child(badge(others.mentions, &chat, font));
        } else if others.messages > 0 {
            row.add_child(dot(chat.sidebar_unread_text, 8.));
        }
        Container::new(
            ConstrainedBox::new(row.finish())
                .with_height(TEAM_HEADER_HEIGHT)
                .finish(),
        )
        .with_horizontal_padding(10.)
        .with_background_color(chat.sidebar_header_bg)
        .finish()
    }

    fn render_team_menu(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let chat = theme.chat();
        let background = theme.surface_1();
        let text = theme.main_text_color(background).into_solid();
        let hover = theme.surface_2();
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (team, mouse_state) in self.teams.iter().zip(&self.team_mouse_states) {
            let current = self.team.as_ref() == Some(&team.id);
            let unread = team.unread;
            let name = team.name.clone();
            let id = team.id.clone();
            let element = Hoverable::new(mouse_state.clone(), move |state| {
                let weight = if unread.is_unread() {
                    Weight::Bold
                } else {
                    Weight::Normal
                };
                let mut row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(
                            1.,
                            Text::new_inline(name, font, appearance.ui_font_size())
                                .with_color(text)
                                .with_style(Properties::default().weight(weight))
                                .finish(),
                        )
                        .finish(),
                    );
                if unread.total_mentions() > 0 {
                    row.add_child(badge(unread.total_mentions(), &chat, font));
                } else if unread.is_unread() {
                    row.add_child(dot(text, 7.));
                }
                let mut container = Container::new(row.finish())
                    .with_horizontal_padding(10.)
                    .with_vertical_padding(7.);
                if current {
                    container = container.with_border(
                        Border::left(3.).with_border_color(chat.sidebar_active_border),
                    );
                }
                if state.is_hovered() {
                    container = container.with_background(hover);
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ChatsAction::SelectTeam(id.clone()))
            })
            .finish();
            column.add_child(element);
        }
        let menu = Container::new(
            ConstrainedBox::new(column.finish())
                .with_width(TEAM_MENU_WIDTH)
                .finish(),
        )
        .with_vertical_padding(4.)
        .with_background(background)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish();
        Dismiss::new(menu)
            .on_dismiss(|ctx, _| ctx.dispatch_typed_action(ChatsAction::CloseTeamMenu))
            .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let rows = self.rows.clone();
        let mouse_states = self.row_mouse_states.clone();
        let font = appearance.ui_font_family();
        let list = UniformList::new(self.list_state.clone(), rows.len(), move |range, app| {
            let chat = Appearance::as_ref(app).theme().chat();
            let rows = rows.clone();
            let mouse_states = mouse_states.clone();
            range.map(move |index| {
                let mouse_state = mouse_states[index].clone();
                render_row(&rows[index], mouse_state, &chat, font)
            })
        });
        let chat = appearance.theme().chat();
        Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            SCROLLBAR_WIDTH,
            fade(chat.sidebar_text, 0.3).into(),
            fade(chat.sidebar_text, 0.5).into(),
            warpui::elements::Fill::None,
        )
        .finish()
    }
}

fn affects_sidebar(delta: &StoreDelta) -> bool {
    !matches!(
        delta,
        StoreDelta::PostsChanged(_)
            | StoreDelta::PostsRemoved(_)
            | StoreDelta::TimelineChanged(_)
            | StoreDelta::ThreadChanged(_)
            | StoreDelta::Typing { .. }
            | StoreDelta::PendingChanged(_)
            | StoreDelta::Fetch(_)
            | StoreDelta::EmojiChanged
    )
}

fn render_row(
    row: &Row,
    mouse_state: MouseStateHandle,
    chat: &ChatColors,
    font: FamilyId,
) -> Box<dyn Element> {
    let chat = *chat;
    let element = match row {
        Row::Header {
            category,
            title,
            collapsed,
        } => {
            let color = fade(chat.sidebar_text, READ_OPACITY);
            let mut content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            if category.is_some() {
                let chevron = if *collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                };
                content.add_child(
                    Container::new(icon(chevron, color, 10.))
                        .with_margin_right(4.)
                        .finish(),
                );
            }
            content.add_child(
                Text::new_inline(title.to_uppercase(), font, 11.)
                    .with_color(color)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            );
            let content = Align::new(content.finish()).left().finish();
            let header = Container::new(content).with_padding_left(ROW_PADDING - 6.);
            match category.clone() {
                Some(category) => Hoverable::new(mouse_state, move |_| header.finish())
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(ChatsAction::ToggleCategory(category.clone()))
                    })
                    .finish(),
                None => header.finish(),
            }
        }
        Row::Channel(channel) => render_channel(channel, mouse_state, chat, font),
        Row::Empty(text) => Container::new(
            Align::new(
                Text::new_inline(text.clone(), font, 12.)
                    .with_color(fade(chat.sidebar_text, READ_OPACITY))
                    .finish(),
            )
            .left()
            .finish(),
        )
        .with_padding_left(ROW_PADDING)
        .finish(),
    };
    ConstrainedBox::new(element)
        .with_height(ROW_HEIGHT)
        .finish()
}

fn render_channel(
    channel: &ChannelRow,
    mouse_state: MouseStateHandle,
    chat: ChatColors,
    font: FamilyId,
) -> Box<dyn Element> {
    let unread = channel.unread.is_unread();
    let text_color = if channel.current {
        chat.sidebar_active_text
    } else if unread {
        chat.sidebar_unread_text
    } else {
        fade(chat.sidebar_text, READ_OPACITY)
    };
    let muted = channel.unread.muted;
    let name = channel.name.clone();
    let icon_kind = channel.icon;
    let mentions = channel.unread.mentions;
    let current = channel.current;
    let id = channel.id.clone();
    Hoverable::new(mouse_state, move |state| {
        let mut text_color = text_color;
        if muted && !current {
            text_color = fade(text_color, 0.6);
        }
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Align::new(channel_icon(icon_kind, text_color, &chat, font)).finish(),
                )
                .with_width(ICON_SIZE + 2.)
                .with_height(ICON_SIZE + 2.)
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.,
                    Container::new(
                        Text::new_inline(name, font, 14.)
                            .with_color(text_color)
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
        if mentions > 0 {
            row.add_child(badge(mentions, &chat, font));
        }
        let mut container = Container::new(row.finish())
            .with_padding_left(ROW_PADDING - 3.)
            .with_padding_right(10.);
        if current {
            container = container
                .with_background_color(chat.sidebar_hover_bg)
                .with_border(Border::left(3.).with_border_color(chat.sidebar_active_border));
        } else {
            container = container
                .with_border(Border::left(3.).with_border_color(ColorU::transparent_black()));
            if state.is_hovered() {
                container = container.with_background_color(chat.sidebar_hover_bg);
            }
        }
        container.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(ChatsAction::Open(id.clone())))
    .finish()
}

pub(crate) fn channel_icon(
    kind: ChannelIcon,
    color: ColorU,
    chat: &ChatColors,
    font: FamilyId,
) -> Box<dyn Element> {
    match kind {
        ChannelIcon::Public => icon(Icon::Hash, color, ICON_SIZE),
        ChannelIcon::Private => icon(Icon::Lock, color, ICON_SIZE - 1.),
        ChannelIcon::Archived => icon(Icon::Archive, color, ICON_SIZE - 1.),
        ChannelIcon::Direct(presence) => {
            let size = 10.;
            match presence {
                Presence::Online => dot(chat.online, size),
                Presence::Away => dot(chat.away, size),
                Presence::Dnd => dot(chat.dnd, size),
                Presence::Offline => ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_border(Border::all(1.5).with_border_color(color))
                        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                        .finish(),
                )
                .with_width(size)
                .with_height(size)
                .finish(),
            }
        }
        ChannelIcon::Group(members) => ConstrainedBox::new(
            Container::new(
                Align::new(
                    Text::new_inline(members.to_string(), font, 9.)
                        .with_color(color)
                        .with_style(Properties::default().weight(Weight::Bold))
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(fade(color, 0.2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
            .finish(),
        )
        .with_width(ICON_SIZE)
        .with_height(ICON_SIZE)
        .finish(),
    }
}

pub(crate) fn icon(icon: Icon, color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(icon.to_warpui_icon(Fill::Solid(color)).finish())
        .with_width(size)
        .with_height(size)
        .finish()
}

fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

/// A mention count pill.
pub(crate) fn badge(count: i64, chat: &ChatColors, font: FamilyId) -> Box<dyn Element> {
    let text = if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    };
    Container::new(
        Text::new_inline(text, font, 11.)
            .with_color(chat.mention_badge_text)
            .with_style(Properties::default().weight(Weight::Bold))
            .finish(),
    )
    .with_horizontal_padding(6.)
    .with_vertical_padding(1.)
    .with_background_color(chat.mention_badge_bg)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
    .finish()
}

/// `color` with its opacity multiplied by `factor`.
pub(crate) fn fade(mut color: ColorU, factor: f32) -> ColorU {
    color.a = (color.a as f32 * factor).round() as u8;
    color
}

impl Entity for ChatsView {
    type Event = ChatsEvent;
}

impl View for ChatsView {
    fn ui_name() -> &'static str {
        "ChatsView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.filter.update(ctx, |filter, ctx| filter.focus(ctx));
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let chat = theme.chat();
        // The field keeps the theme's own colours, like the webapp's "Find channel" box on a
        // coloured sidebar.
        let filter = Container::new(
            Container::new(ChildView::new(&self.filter).finish())
                .with_padding_top(4.)
                .with_horizontal_padding(4.)
                .with_background(theme.background())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .finish(),
        )
        .with_uniform_padding(8.)
        .finish();
        let column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_team_header(&chat, appearance))
            .with_child(filter)
            .with_child(Shrinkable::new(1., self.render_list(appearance)).finish());
        let content = Container::new(column.finish())
            .with_background_color(chat.sidebar_bg)
            .finish();
        let mut stack = Stack::new().with_child(content);
        if self.team_menu_open && self.teams.len() > 1 {
            stack.add_positioned_overlay_child(
                self.render_team_menu(appearance),
                OffsetPositioning::offset_from_parent(
                    vec2f(8., TEAM_HEADER_HEIGHT - 2.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        stack.finish()
    }
}

impl TypedActionView for ChatsView {
    type Action = ChatsAction;

    fn handle_action(&mut self, action: &ChatsAction, ctx: &mut ViewContext<Self>) {
        match action {
            ChatsAction::Open(channel) => self.open(channel.clone(), ctx),
            ChatsAction::ToggleCategory(category) => self.toggle_category(category.clone(), ctx),
            ChatsAction::ToggleTeamMenu => {
                self.team_menu_open = !self.team_menu_open && self.teams.len() > 1;
                ctx.notify();
            }
            ChatsAction::CloseTeamMenu => {
                self.team_menu_open = false;
                ctx.notify();
            }
            ChatsAction::SelectTeam(team) => self.select_team(team.clone(), ctx),
        }
    }
}
