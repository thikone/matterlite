// SPDX-License-Identifier: AGPL-3.0-only
// Layout modelled on warp-terminal `app/src/workspace/view/session_history.rs` @ 5bb7bcc6a
// (search field and toggle bar over date groups of one-line rows); the code is new.

//! The tool panel's History tab (MatterLite.md §10.8): the active chat's loaded messages,
//! one line each, grouped by day (Today and Yesterday open, older days closed, what the user
//! opens or closes kept per chat). The search field matches message text, file names and
//! authors: plain queries through the cache's word index plus a literal scan, regex and
//! case-sensitive ones by scanning. Days with matches open and show their count, the others
//! hide. A click (or Enter on the selection) shows the message in the chat; "Filter the chat"
//! makes the chat show only the matches. The footer says what part of the history is here.
//! "Search older on server" (or Enter with no local match while older history isn't here)
//! runs the query on the server, in the chat; its matches join their days, marked "server".
//! Regex and case-sensitive queries can't (the server's search is word-based).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use chrono::NaiveDate;
use diesel::sqlite::SqliteConnection;
use ml_editor::NavigationKey;
use ml_workspace::{
    Matcher, SearchBarEvent, SearchBarOptions, SearchBarView, ToggleGroup, ToggleSpec,
};
use mm_model::{ChannelId, PostId};
use mm_store::StoreDelta;
use mm_sync::Command;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Empty, Flex, Hoverable, MainAxisSize,
    MouseStateHandle, ParentElement, ScrollStateHandle, Scrollable, ScrollableElement as _,
    Shrinkable, Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats_view::{fade, icon};
use crate::history::{self, OpenDays};
use crate::message::Palette;
use crate::session::{Session, SessionEvent};
use crate::timeline::{ChatLookup, Clock};

const LINE_HEIGHT: f32 = 26.;
const TIME_WIDTH: f32 = 64.;
const AUTHOR_WIDTH: f32 = 84.;
/// Characters kept of a message for its line.
const EXCERPT_CHARS: usize = 200;
/// Word-index hits read per query.
const FTS_LIMIT: i64 = 2000;
const FILTER_TOGGLE: &str = "filter_chat";

/// What the chat shows while "Filter the chat" is on.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatFilter {
    pub channel: ChannelId,
    pub ids: Arc<HashSet<PostId>>,
    pub query: String,
    /// Messages loaded in the chat.
    pub total: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryEvent {
    ShowPost(PostId),
    Filter(Option<ChatFilter>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HistoryAction {
    ToggleDay(NaiveDate),
    Show(PostId),
    LoadOlder,
    SearchServer,
}

/// Where the server pass of a query stands.
#[derive(Clone, Debug, PartialEq)]
enum Server {
    /// "Search older on server".
    Offer,
    Searching,
    /// Matches found on the server (also those already here).
    Found(usize),
    Failed(String),
    /// Regex or case-sensitive matching.
    Unavailable,
}

#[derive(Clone, Debug)]
enum Line {
    Day {
        date: NaiveDate,
        label: String,
        count: usize,
        /// Matches of the query in the day.
        matches: Option<usize>,
        open: bool,
    },
    Post(Box<PostLine>),
    LoadOlder,
    Note(String),
    Server(Server),
}

#[derive(Clone, Debug)]
struct PostLine {
    id: PostId,
    time: String,
    author: String,
    excerpt: String,
    files: usize,
    pinned: bool,
    system: bool,
    /// Found by the server search, not in the loaded history.
    from_server: bool,
}

pub struct HistoryPane {
    search: ViewHandle<SearchBarView>,
    channel: Option<ChannelId>,
    lines: Rc<Vec<Line>>,
    footer: String,
    open: HashMap<ChannelId, OpenDays>,
    /// Matches of the current query in the current chat; `None` without a query.
    matches: Option<Arc<HashSet<PostId>>>,
    /// Messages loaded in the current chat.
    total: usize,
    selected: Option<PostId>,
    excerpts: RefCell<HashMap<PostId, (i64, String)>>,
    reader: Option<SqliteConnection>,
    sent_filter: Option<ChatFilter>,
    /// The (chat, query) whose server search is running.
    searching: Option<(ChannelId, String)>,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    mouse_states: Vec<MouseStateHandle>,
}

impl HistoryPane {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let search = ctx.add_typed_action_view(|ctx| {
            SearchBarView::new(
                SearchBarOptions {
                    placeholder: "Search messages".into(),
                    query_options: true,
                    toggle_groups: vec![
                        ToggleGroup {
                            toggles: vec![ToggleSpec {
                                id: "messages",
                                icon: Icon::MessageChatSquare,
                                tooltip: "Match message text, authors and file names",
                                on: true,
                            }],
                            keep_one_on: true,
                        },
                        ToggleGroup {
                            toggles: vec![ToggleSpec {
                                id: FILTER_TOGGLE,
                                icon: Icon::FilterFunnel,
                                tooltip: "Show only matching messages in the chat",
                                on: false,
                            }],
                            keep_one_on: false,
                        },
                    ],
                },
                ctx,
            )
        });
        ctx.subscribe_to_view(&search, |me, _, event, ctx| match event {
            SearchBarEvent::QueryChanged | SearchBarEvent::Cleared => {
                me.selected = None;
                me.refresh(ctx);
            }
            SearchBarEvent::ToggleChanged { .. } => me.refresh(ctx),
            SearchBarEvent::Submit => {
                match me.selected.clone().or_else(|| me.first_post()) {
                    Some(post) => ctx.emit(HistoryEvent::ShowPost(post)),
                    // Nothing here: ask the server, when older history isn't here.
                    None if me
                        .lines
                        .iter()
                        .any(|line| matches!(line, Line::Server(Server::Offer)))
                        && me.lines.iter().any(|line| matches!(line, Line::LoadOlder)) =>
                    {
                        me.handle_action(&HistoryAction::SearchServer, ctx)
                    }
                    None => {}
                }
            }
            SearchBarEvent::Navigate(NavigationKey::Up) => me.step(-1, ctx),
            SearchBarEvent::Navigate(NavigationKey::Down) => me.step(1, ctx),
            _ => {}
        });
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::Changed(deltas) => {
                if deltas.iter().any(|delta| me.affected_by(delta)) {
                    me.refresh(ctx);
                }
            }
            SessionEvent::Reloaded => me.refresh(ctx),
            _ => {}
        });
        Self {
            search,
            channel: None,
            lines: Rc::default(),
            footer: String::new(),
            open: HashMap::new(),
            matches: None,
            total: 0,
            selected: None,
            excerpts: RefCell::default(),
            reader: None,
            sent_filter: None,
            searching: None,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            mouse_states: Vec::new(),
        }
    }

    pub fn set_channel(&mut self, channel: Option<ChannelId>, ctx: &mut ViewContext<Self>) {
        if channel != self.channel {
            self.channel = channel;
            self.selected = None;
            self.list_state = UniformListState::new();
            self.refresh(ctx);
        }
    }

    /// "Show all" in the chat's filter banner.
    pub fn clear_filter(&mut self, ctx: &mut ViewContext<Self>) {
        self.search.update(ctx, |search, ctx| {
            search.set_toggle(FILTER_TOGGLE, false, ctx)
        });
        self.refresh(ctx);
    }

    fn affected_by(&self, delta: &StoreDelta) -> bool {
        match delta {
            StoreDelta::TimelineChanged(channel)
            | StoreDelta::ChannelChanged(channel)
            | StoreDelta::ServerSearchChanged(channel) => self.channel.as_ref() == Some(channel),
            StoreDelta::PostsChanged(_)
            | StoreDelta::PostsRemoved(_)
            | StoreDelta::UserChanged(_)
            | StoreDelta::NamesChanged
            | StoreDelta::PreferencesChanged => true,
            _ => false,
        }
    }

    fn first_post(&self) -> Option<PostId> {
        self.lines.iter().find_map(|line| match line {
            Line::Post(post) => Some(post.id.clone()),
            _ => None,
        })
    }

    /// Up/Down in the search field move the selection through the message lines.
    fn step(&mut self, delta: isize, ctx: &mut ViewContext<Self>) {
        let posts: Vec<(usize, &PostId)> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| match line {
                Line::Post(post) => Some((index, &post.id)),
                _ => None,
            })
            .collect();
        if posts.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|selected| posts.iter().position(|(_, id)| *id == selected));
        let next = match current {
            None if delta > 0 => 0,
            None => posts.len() - 1,
            Some(current) => (current as isize + delta).clamp(0, posts.len() as isize - 1) as usize,
        };
        let (line, id) = posts[next];
        self.selected = Some(id.clone());
        self.list_state.scroll_to(line);
        ctx.notify();
    }

    /// The posts matching the query in the chat: the cache's word index for plain queries,
    /// and a scan of the loaded messages (literal substrings, authors, regex, case).
    fn find(
        &mut self,
        store: &mm_store::Store,
        channel: &ChannelId,
        posts: &[Arc<mm_model::Post>],
        matcher: &Matcher,
        query: &str,
        dir: Option<std::path::PathBuf>,
    ) -> HashSet<PostId> {
        let mut found = history::scan(store, posts, matcher);
        if matches!(matcher, Matcher::LiteralInsensitive(_)) {
            if self.reader.is_none()
                && let Some(dir) = dir
            {
                self.reader = mm_cache::open_reader(&dir.join("cache.sqlite"))
                    .map_err(|error| log::warn!("history search: {error}"))
                    .ok();
            }
            if let Some(reader) = &mut self.reader {
                match mm_cache::search(reader, query, Some(channel), FTS_LIMIT) {
                    Ok(hits) => {
                        let shown: HashSet<&PostId> = posts.iter().map(|post| &post.id).collect();
                        found.extend(
                            hits.into_iter()
                                .map(|hit| hit.post)
                                .filter(|id| shown.contains(id)),
                        );
                    }
                    Err(error) => log::warn!("history search: {error}"),
                }
            }
        }
        found
    }

    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            self.lines = Rc::default();
            self.footer.clear();
            self.update_filter(ctx);
            ctx.notify();
            return;
        };
        let session = Session::as_ref(ctx);
        let dir = session
            .active()
            .map(|active| active.account_dir().to_owned());
        if !self.open.contains_key(&channel) {
            let saved = session
                .active()
                .and_then(|active| active.ui_state(&open_key(&channel)))
                .map(OpenDays::from_json)
                .unwrap_or_default();
            self.open.insert(channel.clone(), saved);
        }
        let Some(shared) = session.store().cloned() else {
            return;
        };
        let Ok(store) = shared.read() else {
            return;
        };
        let search = self.search.as_ref(ctx);
        let matcher = search.matcher(ctx);
        let query = search.query(ctx);
        let mut posts = history::messages(&store, &channel);
        self.total = posts.len();
        self.matches = matcher
            .as_ref()
            .map(|matcher| Arc::new(self.find(&store, &channel, &posts, matcher, &query, dir)));
        // The server pass: its matches join the local ones (and their days).
        let query_key = (channel.clone(), query.trim().to_owned());
        let plain = matches!(matcher, Some(Matcher::LiteralInsensitive(_)));
        let mut from_server: HashSet<PostId> = HashSet::new();
        let server = match (&matcher, store.server_search.get(&query_key)) {
            (None, _) => None,
            (Some(_), _) if !plain => Some(Server::Unavailable),
            (Some(_), Some(Ok(ids))) => {
                if self.searching.as_ref() == Some(&query_key) {
                    self.searching = None;
                }
                let local: HashSet<PostId> = posts.iter().map(|post| post.id.clone()).collect();
                for id in ids {
                    if let Some(post) = store.posts.get(id) {
                        if !local.contains(id) {
                            from_server.insert(id.clone());
                            posts.push(post.clone());
                        }
                        if let Some(matches) = &mut self.matches {
                            Arc::make_mut(matches).insert(id.clone());
                        }
                    }
                }
                Some(Server::Found(ids.len()))
            }
            (Some(_), Some(Err(error))) => {
                if self.searching.as_ref() == Some(&query_key) {
                    self.searching = None;
                }
                Some(Server::Failed(error.clone()))
            }
            (Some(_), None) if self.searching.as_ref() == Some(&query_key) => {
                Some(Server::Searching)
            }
            (Some(_), None) => Some(Server::Offer),
        };
        if !from_server.is_empty() {
            posts.sort_by_key(|post| std::cmp::Reverse(post.create_at));
        }
        let clock = Clock::of(&store);
        let today = clock.today(now_ms());
        let days = history::days(&posts, &clock, today);
        let lookup = ChatLookup::new(&store, &channel, None);
        let open = self.open.get(&channel).cloned().unwrap_or_default();
        let mut lines = Vec::new();
        for day in &days {
            let matches = self.matches.as_ref().map(|ids| {
                day.posts
                    .iter()
                    .filter(|post| ids.contains(&post.id))
                    .count()
            });
            if matches == Some(0) {
                continue;
            }
            let is_open = matches.is_some() || open.is_open(day.date, today);
            lines.push(Line::Day {
                date: day.date,
                label: day.label.clone(),
                count: day.posts.len(),
                matches,
                open: is_open,
            });
            if !is_open {
                continue;
            }
            for post in &day.posts {
                if let Some(ids) = &self.matches
                    && !ids.contains(&post.id)
                {
                    continue;
                }
                let excerpt = {
                    let mut excerpts = self.excerpts.borrow_mut();
                    match excerpts.get(&post.id) {
                        Some((update_at, text)) if *update_at == post.update_at => text.clone(),
                        _ => {
                            let text = history::excerpt(
                                &mm_render::render_post(post, &lookup.context()).plain_text(),
                                EXCERPT_CHARS,
                            );
                            excerpts.insert(post.id.clone(), (post.update_at, text.clone()));
                            text
                        }
                    }
                };
                lines.push(Line::Post(Box::new(PostLine {
                    id: post.id.clone(),
                    time: clock.time(post.create_at),
                    author: crate::timeline::author(&store, post).name,
                    excerpt,
                    files: post
                        .metadata
                        .as_ref()
                        .map_or(0, |metadata| metadata.files.len()),
                    pinned: post.is_pinned,
                    system: post.is_system(),
                    from_server: from_server.contains(&post.id),
                })));
            }
        }
        let timeline = store.timelines.get(&channel);
        if lines.is_empty() {
            lines.push(Line::Note(match (&self.matches, timeline) {
                (Some(_), _) => "No matches in the local history.".to_owned(),
                (None, None) => "Loading…".to_owned(),
                (None, Some(_)) => "No messages yet.".to_owned(),
            }));
        }
        if let Some(server) = server {
            lines.push(Line::Server(server));
        }
        if timeline.is_some_and(|timeline| {
            !timeline.is_known_empty()
                && !timeline.ranges().first().is_some_and(|range| range.start)
        }) {
            lines.push(Line::LoadOlder);
        }
        let mut footer = history::coverage(&store, &channel, &clock).unwrap_or_default();
        match &self.matches {
            Some(ids) => footer.push_str(&format!(
                " · {} match{}",
                ids.len(),
                if ids.len() == 1 { "" } else { "es" }
            )),
            None => footer.push_str(&format!(
                " · {} message{}",
                posts.len(),
                if posts.len() == 1 { "" } else { "s" }
            )),
        }
        drop(store);
        self.footer = footer;
        while self.mouse_states.len() < lines.len() {
            self.mouse_states.push(MouseStateHandle::default());
        }
        self.lines = Rc::new(lines);
        self.update_filter(ctx);
        ctx.notify();
    }

    /// Tells the chat what to show when "Filter the chat" is on (only on changes).
    fn update_filter(&mut self, ctx: &mut ViewContext<Self>) {
        let on = self.search.as_ref(ctx).is_toggle_on(FILTER_TOGGLE);
        let filter = match (&self.channel, &self.matches) {
            (Some(channel), Some(ids)) if on => Some(ChatFilter {
                channel: channel.clone(),
                ids: ids.clone(),
                query: self.search.as_ref(ctx).query(ctx),
                total: self.total,
            }),
            _ => None,
        };
        if filter != self.sent_filter {
            self.sent_filter = filter.clone();
            ctx.emit(HistoryEvent::Filter(filter));
        }
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let lines = self.lines.clone();
        let mouse_states = self.mouse_states.clone();
        let selected = self.selected.clone();
        let palette = Palette::of(appearance);
        let accent = appearance.theme().accent().into_solid();
        let list = UniformList::new(self.list_state.clone(), lines.len(), move |range, _| {
            let lines = lines.clone();
            let mouse_states = mouse_states.clone();
            let selected = selected.clone();
            let palette = palette.clone();
            range.map(move |index| {
                render_line(
                    &lines[index],
                    mouse_states[index].clone(),
                    selected.as_ref(),
                    &palette,
                    accent,
                )
            })
        });
        let text = appearance
            .theme()
            .main_text_color(appearance.theme().background());
        Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            warpui::elements::ScrollbarWidth::Auto,
            fade(text.into_solid(), 0.25).into(),
            fade(text.into_solid(), 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish()
    }
}

fn open_key(channel: &ChannelId) -> String {
    format!("history_open.{channel}")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn render_line(
    line: &Line,
    mouse_state: MouseStateHandle,
    selected: Option<&PostId>,
    palette: &Palette,
    accent: warpui::color::ColorU,
) -> Box<dyn Element> {
    let font = palette.font;
    let size = palette.size - 2.;
    let element: Box<dyn Element> = match line {
        Line::Day {
            date,
            label,
            count,
            matches,
            open,
        } => {
            let date = *date;
            let mut title = label.clone();
            match matches {
                Some(matches) => title.push_str(&format!(" ({matches} of {count})")),
                None => title.push_str(&format!(" ({count})")),
            }
            let open = *open;
            let palette = palette.clone();
            Hoverable::new(mouse_state, move |state| {
                let chevron = if open { "▾" } else { "▸" };
                let mut container = Container::new(
                    Flex::row()
                        .with_spacing(6.)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Text::new_inline(chevron, font, size)
                                .with_color(palette.muted)
                                .finish(),
                        )
                        .with_child(
                            Text::new_inline(title.clone(), font, size)
                                .with_color(palette.text)
                                .with_style(Properties::default().weight(Weight::Bold))
                                .finish(),
                        )
                        .finish(),
                )
                .with_horizontal_padding(12.)
                .with_vertical_padding(4.);
                if state.is_hovered() {
                    container = container.with_background_color(fade(palette.text, 0.05));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(HistoryAction::ToggleDay(date)))
            .finish()
        }
        Line::Post(post) => {
            let is_selected = selected == Some(&post.id);
            let post = post.clone();
            let palette = palette.clone();
            let id = post.id.clone();
            Hoverable::new(mouse_state, move |state| {
                let text_color = if post.system {
                    palette.muted
                } else {
                    palette.text
                };
                let mut row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(fixed(
                        TIME_WIDTH,
                        Text::new_inline(post.time.clone(), font, size - 1.)
                            .with_color(palette.muted)
                            .finish(),
                    ))
                    .with_child(fixed(
                        AUTHOR_WIDTH,
                        Text::new_inline(post.author.clone(), font, size)
                            .with_color(text_color)
                            .with_style(Properties::default().weight(Weight::Semibold))
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    ))
                    .with_child(
                        Shrinkable::new(
                            1.,
                            Text::new_inline(post.excerpt.clone(), font, size)
                                .with_color(text_color)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    );
                if post.files > 0 {
                    row.add_child(
                        Container::new(icon(Icon::Paperclip, palette.muted, size))
                            .with_margin_left(4.)
                            .finish(),
                    );
                }
                if post.pinned {
                    row.add_child(
                        Container::new(icon(Icon::Pin, palette.muted, size))
                            .with_margin_left(4.)
                            .finish(),
                    );
                }
                if post.from_server {
                    row.add_child(
                        Container::new(
                            Text::new_inline("server", font, size - 2.)
                                .with_color(palette.link)
                                .finish(),
                        )
                        .with_margin_left(6.)
                        .with_horizontal_padding(4.)
                        .with_background_color(fade(palette.link, 0.1))
                        .with_corner_radius(warpui::elements::CornerRadius::with_all(
                            warpui::elements::Radius::Pixels(3.),
                        ))
                        .finish(),
                    );
                }
                let mut container = Container::new(row.finish())
                    .with_padding_left(24.)
                    .with_padding_right(12.)
                    .with_vertical_padding(4.);
                if is_selected {
                    container = container.with_background_color(fade(accent, 0.18));
                } else if state.is_hovered() {
                    container = container.with_background_color(fade(palette.text, 0.05));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(HistoryAction::Show(id.clone())))
            .finish()
        }
        Line::LoadOlder => Hoverable::new(mouse_state, {
            let palette = palette.clone();
            move |_| {
                Container::new(
                    Text::new_inline("Load older messages", font, size)
                        .with_color(palette.link)
                        .finish(),
                )
                .with_horizontal_padding(12.)
                .with_vertical_padding(4.)
                .finish()
            }
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(HistoryAction::LoadOlder))
        .finish(),
        Line::Server(Server::Offer) => Hoverable::new(mouse_state, {
            let palette = palette.clone();
            move |_| {
                Container::new(
                    Text::new_inline("Search older on server", font, size)
                        .with_color(palette.link)
                        .finish(),
                )
                .with_horizontal_padding(12.)
                .with_vertical_padding(4.)
                .finish()
            }
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(HistoryAction::SearchServer))
        .finish(),
        Line::Server(server) => {
            let note = match server {
                Server::Searching => "Searching the server…".to_owned(),
                Server::Found(0) => "Nothing more on the server.".to_owned(),
                Server::Found(1) => "1 match on the server.".to_owned(),
                Server::Found(count) => format!("{count} matches on the server."),
                Server::Failed(error) => format!("Server search failed: {error}"),
                Server::Unavailable => "Regex and case-sensitive searches stay local.".to_owned(),
                Server::Offer => String::new(),
            };
            Container::new(
                Text::new_inline(note, font, size)
                    .with_color(palette.muted)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .with_horizontal_padding(12.)
            .with_vertical_padding(4.)
            .finish()
        }
        Line::Note(note) => Container::new(
            Text::new_inline(note.clone(), font, size)
                .with_color(palette.muted)
                .finish(),
        )
        .with_horizontal_padding(12.)
        .with_vertical_padding(4.)
        .finish(),
    };
    ConstrainedBox::new(element)
        .with_height(LINE_HEIGHT)
        .finish()
}

/// A column exactly `width` wide (see `message::fixed_width`), clipping its text.
fn fixed(width: f32, child: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(crate::message::fixed_width(width, child))
        .with_max_width(width)
        .finish()
}

impl Entity for HistoryPane {
    type Event = HistoryEvent;
}

impl View for HistoryPane {
    fn ui_name() -> &'static str {
        "HistoryPane"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.search.update(ctx, |search, ctx| search.focus(ctx));
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let body = if self.channel.is_some() {
            self.render_list(appearance)
        } else {
            Container::new(
                Text::new(
                    "Open a chat to see its history.",
                    palette.font,
                    palette.size - 1.,
                )
                .with_color(palette.muted)
                .finish(),
            )
            .with_uniform_padding(16.)
            .finish()
        };
        let footer = if self.footer.is_empty() {
            Empty::new().finish()
        } else {
            Container::new(
                Text::new_inline(self.footer.clone(), palette.font, palette.size - 3.)
                    .with_color(palette.muted)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .with_horizontal_padding(12.)
            .with_vertical_padding(6.)
            .with_border(warpui::elements::Border::top(1.).with_border_color(palette.border))
            .finish()
        };
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(ChildView::new(&self.search).finish())
            .with_child(Shrinkable::new(1., body).finish())
            .with_child(footer)
            .finish()
    }
}

impl TypedActionView for HistoryPane {
    type Action = HistoryAction;

    fn handle_action(&mut self, action: &HistoryAction, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        match action {
            HistoryAction::ToggleDay(date) => {
                let today = Session::as_ref(ctx)
                    .store()
                    .and_then(|store| store.read().ok().map(|store| Clock::of(&store)))
                    .map(|clock| clock.today(now_ms()));
                let Some(today) = today else {
                    return;
                };
                if self.matches.is_some() {
                    // While searching, days open by their matches.
                    return;
                }
                let open = self.open.entry(channel.clone()).or_default();
                open.toggle(*date, today);
                let json = open.to_json();
                Session::handle(ctx).update(ctx, |session, _| {
                    session.set_ui_state(&open_key(&channel), Some(json))
                });
                self.refresh(ctx);
            }
            HistoryAction::Show(post) => {
                self.selected = Some(post.clone());
                ctx.emit(HistoryEvent::ShowPost(post.clone()));
                ctx.notify();
            }
            HistoryAction::LoadOlder => Session::as_ref(ctx).send(Command::LoadOlder(channel)),
            HistoryAction::SearchServer => {
                let query = self.search.as_ref(ctx).query(ctx).trim().to_owned();
                if query.is_empty() {
                    return;
                }
                self.searching = Some((channel.clone(), query.clone()));
                Session::as_ref(ctx).send(Command::SearchChannel { channel, query });
                self.refresh(ctx);
            }
        }
    }
}
