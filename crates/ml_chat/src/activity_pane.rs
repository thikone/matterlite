// SPDX-License-Identifier: AGPL-3.0-only

//! The Activity tab of the tool panel (MatterLite.md §10.6): the webapp's Threads view,
//! Recent mentions, Saved messages, Drafts and Scheduled posts, as sub-tabs of the team shown
//! in the Chats tab. A row opens its post: the thread in the right panel, or the chat with the
//! post shown. The lists from the server load when the sub-tab is shown (and on focus).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mm_model::{ChannelId, Post, PostId, TeamId};
use mm_store::{Store, StoreDelta};
use mm_sync::Command;
use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Border, ClippedScrollStateHandle, ClippedScrollable, Container, CornerRadius,
    CrossAxisAlignment, Empty, Expanded, Flex, Highlight, HighlightedRange, Hoverable,
    MainAxisSize, MouseStateHandle, ParentElement, PartialClickableElement as _, Radius,
    Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext,
};

use crate::chats_view::fade;
use crate::message::{MessageRenderer, Palette, Target};
use crate::session::{Session, SessionEvent};
use crate::timeline::{self, ChatLookup, Clock};

const PADDING: f32 = 16.;
/// Characters of a message shown in a thread or draft row.
const EXCERPT: usize = 140;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Threads,
    Mentions,
    Saved,
    Drafts,
    Scheduled,
}

const SECTIONS: [(Section, &str); 5] = [
    (Section::Threads, "Threads"),
    (Section::Mentions, "Mentions"),
    (Section::Saved, "Saved"),
    (Section::Drafts, "Drafts"),
    (Section::Scheduled, "Scheduled"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum ActivityAction {
    Show(Section),
    OpenPost(PostId),
    OpenThread(PostId),
    OpenChat { channel: ChannelId, root: PostId },
    DeleteDraft { channel: ChannelId, root: PostId },
    DeleteScheduled(String),
    MarkAllRead,
    Click(Target),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// What to open.
pub enum ActivityEvent {
    /// A mention or saved post: its thread (CRT reply) or its chat with the post shown.
    Post(PostId),
    Thread(PostId),
    /// A draft or scheduled post: its chat, or its thread when `root` isn't empty.
    Chat {
        channel: ChannelId,
        root: PostId,
    },
}

pub struct ActivityPane {
    team: Option<TeamId>,
    section: Section,
    section_states: Vec<MouseStateHandle>,
    /// Hover states of the rows and their links, by a key per row.
    row_states: RefCell<HashMap<String, MouseStateHandle>>,
    scroll_state: ClippedScrollStateHandle,
    /// The saved posts the list was loaded for (it reloads when they change).
    saved_loaded_for: Option<HashSet<String>>,
}

impl ActivityPane {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::Changed(deltas) => me.on_changes(deltas, ctx),
            SessionEvent::Reloaded | SessionEvent::DraftsChanged => ctx.notify(),
            _ => {}
        });
        Self {
            team: None,
            section: Section::Threads,
            section_states: SECTIONS
                .iter()
                .map(|_| MouseStateHandle::default())
                .collect(),
            row_states: RefCell::new(HashMap::new()),
            scroll_state: ClippedScrollStateHandle::default(),
            saved_loaded_for: None,
        }
    }

    /// The team whose threads, drafts and scheduled posts are listed.
    pub fn set_team(&mut self, team: Option<TeamId>, ctx: &mut ViewContext<Self>) {
        if team != self.team {
            self.team = team;
            if matches!(self.section, Section::Scheduled) {
                self.load(ctx);
            }
            ctx.notify();
        }
    }

    /// Asks the server for the shown list (mentions, saved, scheduled).
    fn load(&mut self, ctx: &mut ViewContext<Self>) {
        let session = Session::as_ref(ctx);
        match self.section {
            Section::Mentions => session.send(Command::LoadMentions),
            Section::Saved => {
                self.saved_loaded_for = session
                    .store()
                    .and_then(|store| Some(saved_ids(&*store.read().ok()?)));
                session.send(Command::LoadSaved);
            }
            Section::Scheduled => {
                let enabled = session
                    .store()
                    .and_then(|store| Some(store.read().ok()?.scheduled_posts_enabled()))
                    .unwrap_or(false);
                if let (true, Some(team)) = (enabled, self.team.clone()) {
                    session.send(Command::LoadScheduled(team));
                }
            }
            Section::Threads | Section::Drafts => {}
        }
    }

    fn on_changes(&mut self, deltas: &[StoreDelta], ctx: &mut ViewContext<Self>) {
        let relevant = deltas.iter().any(|delta| {
            !matches!(
                delta,
                StoreDelta::Typing { .. } | StoreDelta::StatusChanged(_) | StoreDelta::Fetch(_)
            )
        });
        // Saved or unsaved elsewhere: the Saved list follows.
        if self.section == Section::Saved
            && deltas.contains(&StoreDelta::PreferencesChanged)
            && let Some(now) = Session::as_ref(ctx)
                .store()
                .and_then(|store| Some(saved_ids(&*store.read().ok()?)))
            && self.saved_loaded_for.as_ref() != Some(&now)
        {
            self.load(ctx);
        }
        if relevant {
            ctx.notify();
        }
    }

    fn row_state(&self, key: String) -> MouseStateHandle {
        self.row_states.borrow_mut().entry(key).or_default().clone()
    }

    fn render_sections(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = theme.main_text_color(theme.background()).into_solid();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let accent = theme.accent().into_solid();
        let mut row = Flex::row().with_spacing(2.);
        for (index, (section, label)) in SECTIONS.into_iter().enumerate() {
            let active = section == self.section;
            let button = Hoverable::new(self.section_states[index].clone(), move |state| {
                let mut container = Container::new(
                    Text::new_inline(label, font, size - 1.)
                        .with_color(if active { text } else { fade(text, 0.7) })
                        .with_style(Properties::default().weight(if active {
                            Weight::Bold
                        } else {
                            Weight::Normal
                        }))
                        .finish(),
                )
                .with_horizontal_padding(6.)
                .with_vertical_padding(6.);
                if active {
                    container = container.with_border(Border::bottom(2.).with_border_color(accent));
                } else if state.is_hovered() {
                    container = container.with_background_color(fade(text, 0.06));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(ActivityAction::Show(section)))
            .finish();
            row.add_child(button);
        }
        Container::new(row.finish())
            .with_horizontal_padding(PADDING - 8.)
            .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
            .finish()
    }

    /// A clickable card.
    fn card(
        &self,
        key: String,
        content: Box<dyn Element>,
        action: ActivityAction,
        palette: &Palette,
    ) -> Box<dyn Element> {
        let (border, text) = (palette.border, palette.text);
        Hoverable::new(self.row_state(key), move |state| {
            let mut container = Container::new(content)
                .with_uniform_padding(10.)
                .with_border(Border::all(1.).with_border_color(border))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
            if state.is_hovered() {
                container = container.with_background_color(fade(text, 0.04));
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
        .finish()
    }

    fn render_threads(&self, store: &Store, palette: &Palette) -> Box<dyn Element> {
        if !store.crt_enabled() {
            return note(
                "Followed threads are listed with Collapsed Reply Threads on (Settings › \
                 Display in the webapp).",
                palette,
            );
        }
        let clock = Clock::of(store);
        let today = clock.today(now_ms());
        let mut threads: Vec<_> = store
            .thread_states
            .values()
            .filter(|thread| thread.delete_at == 0)
            .filter_map(|thread| Some((thread, store.posts.get(&thread.id)?)))
            .filter(|(_, root)| in_team(store, &root.channel_id, self.team.as_ref()))
            .collect();
        if threads.is_empty() {
            return note(
                "No followed threads yet. Replying to a thread or following it lists it here.",
                palette,
            );
        }
        threads.sort_by_key(|(thread, _)| {
            let unread = thread.unread_replies > 0 || thread.unread_mentions > 0;
            (!unread, -thread.last_reply_at)
        });
        let any_unread = threads
            .iter()
            .any(|(thread, _)| thread.unread_replies > 0 || thread.unread_mentions > 0);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        if any_unread {
            column.add_child(link_line(
                "Mark all as read",
                ActivityAction::MarkAllRead,
                palette,
            ));
        }
        for (thread, root) in threads {
            let unread = thread.unread_replies > 0 || thread.unread_mentions > 0;
            let channel = store.channel_name(&root.channel_id).unwrap_or_default();
            let author = timeline::author(store, root).name;
            let participants: Vec<String> = thread
                .participants
                .iter()
                .take(3)
                .map(|user| store.user_name(&user.id).unwrap_or(user.username.clone()))
                .collect();
            let mut details = vec![timeline::replies_label(thread.reply_count.max(0) as usize)];
            if !participants.is_empty() {
                details.push(participants.join(", "));
            }
            if thread.last_reply_at > 0 {
                details.push(format!(
                    "last reply {} {}",
                    timeline::date_label(clock.zone.date(thread.last_reply_at), today),
                    clock.time(thread.last_reply_at)
                ));
            }
            let mut top = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.)
                .with_child(
                    Expanded::new(1., small(channel, palette.muted, palette).finish()).finish(),
                );
            if thread.unread_mentions > 0 {
                top.add_child(pill(format!("@{}", thread.unread_mentions), palette));
            } else if unread {
                top.add_child(pill("New".to_owned(), palette));
            }
            let content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.)
                .with_child(top.finish())
                .with_child(
                    Text::new(
                        format!("{author}: {}", excerpt(&root.message)),
                        palette.font,
                        palette.size - 1.,
                    )
                    .with_color(palette.text)
                    .with_style(Properties::default().weight(if unread {
                        Weight::Bold
                    } else {
                        Weight::Normal
                    }))
                    .finish(),
                )
                .with_child(small(details.join(" · "), palette.muted, palette).finish())
                .finish();
            column.add_child(self.card(
                format!("thread:{}", thread.id),
                content,
                ActivityAction::OpenThread(thread.id.clone()),
                palette,
            ));
        }
        column.finish()
    }

    /// Mentions or saved posts: cards like the Channel tab's pinned messages.
    fn render_posts(
        &self,
        store: &Store,
        ids: Option<&Vec<PostId>>,
        empty: &str,
        palette: &Palette,
    ) -> Box<dyn Element> {
        let Some(ids) = ids else {
            return note("Loading…", palette);
        };
        let posts: Vec<&std::sync::Arc<Post>> = ids
            .iter()
            .filter_map(|id| store.posts.get(id))
            .filter(|post| !post.is_deleted())
            .collect();
        if posts.is_empty() {
            return note(empty, palette);
        }
        let clock = Clock::of(store);
        let today = clock.today(now_ms());
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        for post in posts {
            let team = store
                .channels
                .get(&post.channel_id)
                .map(|channel| channel.team_id.clone())
                .filter(|team| !team.is_empty());
            let lookup = ChatLookup::new(store, &post.channel_id, team.as_ref());
            let doc = Rc::new(mm_render::render_post(post, &lookup.context()));
            let renderer = MessageRenderer {
                palette,
                custom_emoji: &Default::default(),
                on_click: Rc::new(|target, ctx| {
                    ctx.dispatch_typed_action(ActivityAction::Click(target))
                }),
                muted: false,
            };
            let channel = store.channel_name(&post.channel_id).unwrap_or_default();
            let when = format!(
                "{} {}",
                timeline::date_label(clock.zone.date(post.create_at), today),
                clock.time(post.create_at)
            );
            let content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.)
                .with_child(small(format!("{channel} · {when}"), palette.muted, palette).finish())
                .with_child(
                    Text::new_inline(
                        timeline::author(store, post).name,
                        palette.font,
                        palette.size - 1.,
                    )
                    .with_color(palette.text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
                )
                .with_child(renderer.render(&doc))
                .finish();
            column.add_child(self.card(
                format!("post:{}", post.id),
                content,
                ActivityAction::OpenPost(post.id.clone()),
                palette,
            ));
        }
        column.finish()
    }

    fn render_drafts(
        &self,
        store: &Store,
        app: &AppContext,
        palette: &Palette,
    ) -> Box<dyn Element> {
        let Some(active) = Session::as_ref(app).active() else {
            return Empty::new().finish();
        };
        let mut drafts: Vec<(String, ChannelId, PostId, String)> = active
            .drafts()
            .filter(|((channel, _), _)| in_team(store, channel, self.team.as_ref()))
            .map(|((channel, root), text)| {
                let name = store
                    .channel_name(channel)
                    .unwrap_or_else(|| channel.to_string());
                (name, channel.clone(), root.clone(), text.clone())
            })
            .collect();
        if drafts.is_empty() {
            return note(
                "No drafts. Text left in a composer stays here until it is sent.",
                palette,
            );
        }
        drafts.sort();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        for (name, channel, root, text) in drafts {
            let place = if root.is_empty() {
                name
            } else {
                format!("{name} · Thread")
            };
            let delete = ActivityAction::DeleteDraft {
                channel: channel.clone(),
                root: root.clone(),
            };
            let content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.)
                .with_child(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_child(
                            Expanded::new(1., small(place, palette.muted, palette).finish())
                                .finish(),
                        )
                        .with_child(link_text("Delete", delete, palette))
                        .finish(),
                )
                .with_child(
                    Text::new(excerpt(&text), palette.font, palette.size - 1.)
                        .with_color(palette.text)
                        .finish(),
                )
                .finish();
            column.add_child(self.card(
                format!("draft:{channel}:{root}"),
                content,
                ActivityAction::OpenChat { channel, root },
                palette,
            ));
        }
        column.finish()
    }

    fn render_scheduled(
        &self,
        store: &Store,
        palette: &Palette,
        error: warpui::color::ColorU,
    ) -> Box<dyn Element> {
        if !store.scheduled_posts_enabled() {
            return note(
                "Scheduled messages need a licensed server with scheduled posts turned on.",
                palette,
            );
        }
        let Some(team) = &self.team else {
            return Empty::new().finish();
        };
        let Some(posts) = store.scheduled.get(team) else {
            return note("Loading…", palette);
        };
        if posts.is_empty() {
            return note(
                "No scheduled messages. Messages scheduled in the webapp (or a mobile app) \
                 are listed here.",
                palette,
            );
        }
        let clock = Clock::of(store);
        let today = clock.today(now_ms());
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        for post in posts {
            let name = store
                .channel_name(&post.channel_id)
                .unwrap_or_else(|| post.channel_id.to_string());
            let (status, color) = if post.error_code.is_empty() {
                (
                    format!(
                        "Send on {} at {}",
                        timeline::date_label(clock.zone.date(post.scheduled_at), today),
                        clock.time(post.scheduled_at)
                    ),
                    palette.muted,
                )
            } else {
                (
                    format!("Not sent: {}", post.error_code.replace('_', " ")),
                    error,
                )
            };
            let content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.)
                .with_child(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_child(
                            Expanded::new(1., small(name, palette.muted, palette).finish())
                                .finish(),
                        )
                        .with_child(link_text(
                            "Delete",
                            ActivityAction::DeleteScheduled(post.id.clone()),
                            palette,
                        ))
                        .finish(),
                )
                .with_child(small(status, color, palette).finish())
                .with_child(
                    Text::new(excerpt(&post.message), palette.font, palette.size - 1.)
                        .with_color(palette.text)
                        .finish(),
                )
                .finish();
            column.add_child(self.card(
                format!("scheduled:{}", post.id),
                content,
                ActivityAction::OpenChat {
                    channel: post.channel_id.clone(),
                    root: post.root_id.clone(),
                },
                palette,
            ));
        }
        column.finish()
    }
}

/// My saved posts according to the `flagged_post` preferences.
fn saved_ids(store: &Store) -> HashSet<String> {
    store
        .preferences
        .iter()
        .filter(|((category, _), value)| {
            category == mm_model::preference::keys::FLAGGED_POST && *value == "true"
        })
        .map(|((_, name), _)| name.clone())
        .collect()
}

/// Whether a channel belongs to the team (DMs and GMs belong to every team).
fn in_team(store: &Store, channel: &ChannelId, team: Option<&TeamId>) -> bool {
    match (store.channels.get(channel), team) {
        (Some(channel), Some(team)) => channel.team_id.is_empty() || channel.team_id == *team,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The first lines of a message as one line, cut with an ellipsis.
fn excerpt(message: &str) -> String {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= EXCERPT {
        return line;
    }
    let cut: String = line.chars().take(EXCERPT).collect();
    format!("{}…", cut.trim_end())
}

fn small(text: String, color: warpui::color::ColorU, palette: &Palette) -> Text {
    Text::new_inline(text, palette.font, palette.size - 2.)
        .with_color(color)
        .with_clip(ClipConfig::ellipsis())
}

fn note(text: &str, palette: &Palette) -> Box<dyn Element> {
    Text::new(text.to_owned(), palette.font, palette.size - 1.)
        .with_color(palette.muted)
        .finish()
}

fn pill(text: String, palette: &Palette) -> Box<dyn Element> {
    Container::new(
        Text::new_inline(text, palette.font, palette.size - 3.)
            .with_color(palette.link)
            .with_style(Properties::default().weight(Weight::Bold))
            .finish(),
    )
    .with_horizontal_padding(5.)
    .with_background_color(fade(palette.link, 0.12))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .finish()
}

/// A link inside a card (its click doesn't open the card).
fn link_text(label: &'static str, action: ActivityAction, palette: &Palette) -> Box<dyn Element> {
    let length = label.chars().count();
    Text::new_inline(label, palette.font, palette.size - 2.)
        .with_color(palette.link)
        .with_highlights([HighlightedRange {
            highlight: Highlight::new().with_foreground_color(palette.link),
            highlight_indices: (0..length).collect(),
        }])
        .with_clickable_char_range(0..length, move |_, ctx, _| {
            ctx.dispatch_typed_action(action.clone())
        })
        .finish()
}

fn link_line(label: &'static str, action: ActivityAction, palette: &Palette) -> Box<dyn Element> {
    Flex::row()
        .with_child(link_text(label, action, palette))
        .finish()
}

impl Entity for ActivityPane {
    type Event = ActivityEvent;
}

impl View for ActivityPane {
    fn ui_name() -> &'static str {
        "ActivityPane"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.load(ctx);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let session = Session::as_ref(app);
        let body = match session.store().and_then(|store| store.read().ok()) {
            None => Empty::new().finish(),
            Some(store) => match self.section {
                Section::Threads => self.render_threads(&store, &palette),
                Section::Mentions => self.render_posts(
                    &store,
                    store.mentions.as_ref(),
                    "No mentions yet. Messages that mention you or your keywords are listed \
                     here.",
                    &palette,
                ),
                Section::Saved => self.render_posts(
                    &store,
                    store.saved.as_ref(),
                    "No saved messages yet. Save a message from its menu to find it here.",
                    &palette,
                ),
                Section::Drafts => self.render_drafts(&store, app, &palette),
                Section::Scheduled => {
                    self.render_scheduled(&store, &palette, appearance.theme().chat().error_text)
                }
            },
        };
        let text = palette.text;
        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            Container::new(body).with_uniform_padding(PADDING).finish(),
            warpui::elements::ScrollbarWidth::Auto,
            fade(text, 0.25).into(),
            fade(text, 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_sections(appearance))
            .with_child(Shrinkable::new(1., scrollable).finish())
            .finish()
    }
}

impl TypedActionView for ActivityPane {
    type Action = ActivityAction;

    fn handle_action(&mut self, action: &ActivityAction, ctx: &mut ViewContext<Self>) {
        match action {
            ActivityAction::Show(section) => {
                self.section = *section;
                self.scroll_state.scroll_to(warpui::units::Pixels::zero());
                self.load(ctx);
                ctx.notify();
            }
            ActivityAction::OpenPost(post) => ctx.emit(ActivityEvent::Post(post.clone())),
            ActivityAction::OpenThread(root) => ctx.emit(ActivityEvent::Thread(root.clone())),
            ActivityAction::OpenChat { channel, root } => ctx.emit(ActivityEvent::Chat {
                channel: channel.clone(),
                root: root.clone(),
            }),
            ActivityAction::DeleteDraft { channel, root } => {
                let (channel, root) = (channel.clone(), root.clone());
                Session::handle(ctx).update(ctx, |session, ctx| {
                    session.set_draft(&channel, &root, String::new(), ctx)
                });
            }
            ActivityAction::DeleteScheduled(id) => {
                if let Some(team) = self.team.clone() {
                    Session::as_ref(ctx).send(Command::DeleteScheduled {
                        team,
                        id: id.clone(),
                    });
                }
            }
            ActivityAction::MarkAllRead => {
                if let Some(team) = self.team.clone() {
                    Session::as_ref(ctx).send(Command::MarkAllThreadsRead(team));
                }
            }
            ActivityAction::Click(Target::Url(url)) => ctx.open_url(url),
            ActivityAction::Click(_) => {}
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
#[path = "activity_pane_tests.rs"]
mod tests;
