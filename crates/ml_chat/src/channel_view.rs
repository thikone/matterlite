// SPDX-License-Identifier: AGPL-3.0-only

//! A chat tab's content (MatterLite.md §10.2, §10.4): the channel header and the timeline,
//! anchored at the bottom, with date separators, the "New messages" line, combined
//! membership messages, posts with avatars, holes that load older or newer posts when they
//! come near, pending and failed posts with Retry and Delete, and "Jump to newest".
//!
//! Each open tab keeps its own scroll position. Rows are prepared from the store when it
//! changes; messages are rendered (`mm_render`) only when they scroll into view and kept
//! until the post or what rendering depends on changes; row heights are kept by the list
//! until the row's content, the width or the theme changes.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ml_themes::ChatColors;
use mm_model::{ChannelId, ChannelType, Post, PostId, TeamId};
use mm_render::MessageDoc;
use mm_store::{Store, StoreDelta, dm_partner};
use mm_sync::{Command, ImageKey};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::assets::asset_cache::AssetSource;
use warpui::color::ColorU;
use warpui::elements::ChildView;
use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    DispatchEventResult, Empty, EventHandler, Expanded, Flex, Highlight, HighlightedRange,
    Hoverable, Image, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement,
    PartialClickableElement as _, Radius, Rect, ScrollStateHandle, Scrollable,
    ScrollableElement as _, Shrinkable, Stack, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::image_cache::CacheOption;
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats::{ChannelIcon, channel_icon as icon_kind};
use crate::chats_view::{channel_icon, fade, icon};
use crate::composer;
use crate::composer_view::{ComposerEvent, ComposerView};
use crate::history_pane::ChatFilter;
use crate::message::{
    CustomEmoji, MessageRenderer, OnClick, Palette, Target, emoji_image, fixed_width,
};
use crate::session::{Session, SessionEvent};
use crate::timeline::{self, Author, ChatLookup, Clock, HoleKind, Pending, Row};
use crate::timeline_list::{TimelineList, TimelineState, Viewport};
use mm_render::EmojiRef;
use warpui::text_layout::ClipConfig;

const HEADER_HEIGHT: f32 = 44.;
const AVATAR: f32 = 32.;
/// The avatar column of a post.
const GUTTER: f32 = AVATAR + 12.;
const SIDE_PADDING: f32 = 20.;
/// Rows from either end of the loaded posts at which the next page is asked for.
const LOAD_MARGIN: usize = 4;
/// A load that brought nothing back may be asked for again after this.
const LOAD_RETRY: Duration = Duration::from_secs(10);
/// The "New messages" line, this far below the top when a chat opens there.
const NEW_LINE_OFFSET: f32 = 80.;
/// A post shown from elsewhere: this far below the top, highlighted this long.
const SHOW_POST_OFFSET: f32 = 120.;
const FLASH: Duration = Duration::from_secs(3);
const SCROLLBAR: warpui::elements::ScrollbarWidth = warpui::elements::ScrollbarWidth::Auto;

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelAction {
    Click(Target),
    Load(HoleKind),
    JumpToNewest,
    /// `Esc`: mark the chat read now.
    MarkRead,
    /// "Show all" in the filter banner.
    ClearFilter,
    /// A click in the chat: it takes the keyboard (PageUp/PageDown, Esc).
    Focus,
    Retry(String),
    Discard(String),
    PageUp,
    PageDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEvent {
    /// A `~channel` link was clicked.
    OpenChannel(ChannelId),
    /// "Show all" in the filter banner: turn "Filter the chat" off.
    ClearFilter,
}

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;
    app.register_fixed_bindings([
        FixedBinding::new("escape", ChannelAction::MarkRead, id!("ChannelView")),
        FixedBinding::new("pageup", ChannelAction::PageUp, id!("ChannelView")),
        FixedBinding::new("pagedown", ChannelAction::PageDown, id!("ChannelView")),
        FixedBinding::new(
            "cmdorctrl-end",
            ChannelAction::JumpToNewest,
            id!("ChannelView"),
        ),
    ]);
}

/// A chat open in a tab.
struct Chat {
    list: TimelineState,
    /// `last_viewed_at` when the chat was opened: where the "New messages" line goes.
    last_viewed_at: Option<i64>,
    /// Scroll to the "New messages" line as soon as it is there.
    jump_to_new: bool,
    viewport: Viewport,
    /// When older or newer posts were last asked for.
    loading_older: Option<Instant>,
    loading_newer: Option<Instant>,
}

/// A row ready to draw.
enum RowData {
    Start { title: String, subtitle: String },
    Date(String),
    NewMessages,
    Post(Box<PostData>),
    Activity { text: String, time: String },
    Hole(HoleKind),
}

struct PostData {
    post: Arc<Post>,
    consecutive: bool,
    pending: Option<Pending>,
    author: Author,
    avatar: Option<PathBuf>,
    time: String,
    /// Emoji name, count, and whether one of them is mine.
    reactions: Vec<(String, usize, bool)>,
    files: Vec<String>,
    /// Replies of a root post under Collapsed Reply Threads.
    replies: i64,
    /// A reply shown in the channel (CRT off).
    is_reply: bool,
}

/// Messages rendered so far, by post (and `update_at`).
#[derive(Default)]
struct Docs {
    posts: HashMap<PostId, (i64, Rc<MessageDoc>)>,
    activity: HashMap<String, Rc<MessageDoc>>,
}

struct Shared {
    rows: Vec<RowData>,
    lookup: Rc<ChatLookup>,
    docs: Rc<RefCell<Docs>>,
    custom_emoji: Rc<CustomEmoji>,
}

pub struct ChannelView {
    channel: Option<ChannelId>,
    chats: HashMap<ChannelId, Chat>,
    shared: Rc<Shared>,
    /// The channel's name, icon and header line, and its member count.
    title: String,
    icon: Option<ChannelIcon>,
    header: String,
    members: Option<i64>,
    lookup: Rc<ChatLookup>,
    lookup_dirty: bool,
    docs: Rc<RefCell<Docs>>,
    /// Images on disk, and images asked for this session.
    images: HashSet<ImageKey>,
    requested: HashSet<ImageKey>,
    account_dir: Option<PathBuf>,
    custom_emoji: Rc<CustomEmoji>,
    /// The theme the row heights were measured in.
    theme_generation: u64,
    scroll_state: ScrollStateHandle,
    jump_mouse_state: MouseStateHandle,
    composer: ViewHandle<ComposerView>,
    /// History's "Filter the chat": only these posts are shown.
    filter: Option<ChatFilter>,
    /// A post shown from elsewhere (History, Pinned), highlighted for a moment.
    flash: Option<PostId>,
    flashes: u64,
    /// The next focus of this view stays on the timeline (a click in it) instead of moving
    /// on to the composer.
    focus_timeline: bool,
}

impl ChannelView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.theme_generation += 1;
            for chat in me.chats.values() {
                chat.list.clear_heights();
            }
            ctx.notify();
        });
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::Changed(deltas) => me.on_changes(deltas, ctx),
            SessionEvent::Reloaded => {
                me.lookup_dirty = true;
                me.refresh(ctx);
            }
            SessionEvent::ChannelOpened {
                channel,
                last_viewed_at,
            } => {
                log::debug!("channel opened: {channel} last viewed {last_viewed_at}");
                if let Some(chat) = me.chats.get_mut(channel) {
                    chat.last_viewed_at = Some(*last_viewed_at);
                    chat.jump_to_new = true;
                    if me.channel.as_ref() == Some(channel) {
                        me.refresh(ctx);
                    }
                }
            }
            SessionEvent::ImageReady(key) => me.on_image_ready(key.clone(), ctx),
            SessionEvent::TypingChanged => ctx.notify(),
            _ => {}
        });
        let account_dir = Session::as_ref(ctx)
            .active()
            .map(|active| active.account_dir().to_owned());
        let composer = ctx.add_typed_action_view(ComposerView::new);
        ctx.subscribe_to_view(&composer, |me, _, event, ctx| match event {
            ComposerEvent::Sent => {
                // My post shows at the bottom, as in the webapp.
                if let Some(chat) = me
                    .channel
                    .as_ref()
                    .and_then(|channel| me.chats.get(channel))
                {
                    chat.list.scroll_to_bottom();
                }
                ctx.notify();
            }
            ComposerEvent::Escape => me.handle_action(&ChannelAction::MarkRead, ctx),
        });
        let lookup = Rc::new(ChatLookup::default());
        let docs = Rc::new(RefCell::new(Docs::default()));
        let custom_emoji = Rc::new(CustomEmoji::new());
        Self {
            channel: None,
            chats: HashMap::new(),
            shared: Rc::new(Shared {
                rows: Vec::new(),
                lookup: lookup.clone(),
                docs: docs.clone(),
                custom_emoji: custom_emoji.clone(),
            }),
            title: String::new(),
            icon: None,
            header: String::new(),
            members: None,
            lookup,
            lookup_dirty: true,
            docs,
            images: HashSet::new(),
            requested: HashSet::new(),
            account_dir,
            custom_emoji,
            theme_generation: 0,
            scroll_state: ScrollStateHandle::default(),
            jump_mouse_state: MouseStateHandle::default(),
            composer,
            filter: None,
            flash: None,
            flashes: 0,
            focus_timeline: false,
        }
    }

    /// Shows only the matches of History's search in their chat, or everything again.
    pub fn set_filter(&mut self, filter: Option<ChatFilter>, ctx: &mut ViewContext<Self>) {
        if filter != self.filter {
            self.filter = filter;
            self.refresh(ctx);
        }
    }

    /// The filter, if it is for the chat on screen.
    fn active_filter(&self) -> Option<&ChatFilter> {
        self.filter
            .as_ref()
            .filter(|filter| Some(&filter.channel) == self.channel.as_ref())
    }

    /// Scrolls the open chat to a post and highlights it for a moment (the webapp's permalink
    /// view). False when the post isn't among the loaded rows.
    pub fn show_post(&mut self, post: &PostId, ctx: &mut ViewContext<Self>) -> bool {
        let key = timeline::RowKey::Post(post.clone());
        let found = self
            .shared
            .rows
            .iter()
            .any(|row| matches!(row, RowData::Post(data) if data.post.id == *post));
        if !found {
            return false;
        }
        let Some(chat) = self
            .channel
            .clone()
            .and_then(|channel| self.chats.get_mut(&channel))
        else {
            return false;
        };
        chat.list.scroll_to(key, SHOW_POST_OFFSET);
        // Showing a post replaces the jump to the "New messages" line.
        chat.jump_to_new = false;
        self.flash = Some(post.clone());
        self.flashes += 1;
        let flashes = self.flashes;
        let _ = ctx.spawn(
            async { warpui::r#async::Timer::after(FLASH).await },
            move |me, _, ctx| {
                if me.flashes == flashes {
                    me.flash = None;
                    ctx.notify();
                }
            },
        );
        ctx.notify();
        true
    }

    /// Shows a chat (the active tab's), or nothing.
    pub fn set_channel(&mut self, channel: Option<ChannelId>, ctx: &mut ViewContext<Self>) {
        if channel == self.channel {
            return;
        }
        if let Some(channel) = &channel
            && !self.chats.contains_key(channel)
        {
            let (list, viewports) = TimelineState::new();
            let id = channel.clone();
            ctx.spawn_stream_local(
                viewports,
                move |me, viewport, ctx| me.on_viewport(&id, viewport, ctx),
                |_, _| {},
            );
            self.chats.insert(
                channel.clone(),
                Chat {
                    list,
                    last_viewed_at: None,
                    jump_to_new: true,
                    viewport: Viewport::default(),
                    loading_older: None,
                    loading_newer: None,
                },
            );
        }
        self.channel = channel.clone();
        self.lookup_dirty = true;
        self.composer
            .update(ctx, |composer, ctx| composer.set_channel(channel, ctx));
        self.refresh(ctx);
    }

    /// Forgets the chats whose tabs closed.
    pub fn retain(&mut self, open: &[ChannelId]) {
        self.chats.retain(|channel, _| open.contains(channel));
    }

    fn on_changes(&mut self, deltas: &[StoreDelta], ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let mut relevant = false;
        for delta in deltas {
            match delta {
                StoreDelta::TimelineChanged(changed) => {
                    if *changed == channel {
                        relevant = true;
                        if let Some(chat) = self.chats.get_mut(&channel) {
                            chat.loading_older = None;
                            chat.loading_newer = None;
                        }
                    }
                }
                StoreDelta::PostsChanged(_)
                | StoreDelta::PostsRemoved(_)
                | StoreDelta::PendingChanged(_) => relevant = true,
                StoreDelta::RolesChanged => {
                    self.composer
                        .update(ctx, |composer, ctx| composer.update_state(ctx));
                }
                StoreDelta::ChannelChanged(_)
                | StoreDelta::MemberChanged(_)
                | StoreDelta::UserChanged(_)
                | StoreDelta::NamesChanged
                | StoreDelta::PreferencesChanged
                | StoreDelta::EmojiChanged
                | StoreDelta::ConfigChanged
                | StoreDelta::CrtChanged => {
                    relevant = true;
                    self.lookup_dirty = true;
                }
                _ => {}
            }
        }
        if relevant {
            self.composer
                .update(ctx, |composer, ctx| composer.update_state(ctx));
            self.refresh(ctx);
        }
    }

    fn on_image_ready(&mut self, key: ImageKey, ctx: &mut ViewContext<Self>) {
        if !self.requested.contains(&key) {
            return;
        }
        if let ImageKey::Emoji(_) = key {
            // Custom emoji change the layout of their messages.
            for chat in self.chats.values() {
                chat.list.clear_heights();
            }
        }
        self.images.insert(key);
        self.refresh(ctx);
    }

    fn on_viewport(
        &mut self,
        channel: &ChannelId,
        viewport: Viewport,
        ctx: &mut ViewContext<Self>,
    ) {
        let at_bottom_changed = self
            .chats
            .get(channel)
            .is_some_and(|chat| chat.viewport.at_bottom != viewport.at_bottom);
        if let Some(chat) = self.chats.get_mut(channel) {
            chat.viewport = viewport;
        }
        if self.channel.as_ref() == Some(channel) {
            self.load_if_near_an_end(ctx);
            if at_bottom_changed {
                ctx.notify();
            }
        }
    }

    /// Asks for the next page when a loadable hole is within a few rows of the screen.
    fn load_if_near_an_end(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let rows = &self.shared.rows;
        let Some(chat) = self.chats.get_mut(&channel) else {
            return;
        };
        if rows.is_empty() || chat.viewport.rows != rows.len() {
            return;
        }
        let first = chat.viewport.first.saturating_sub(LOAD_MARGIN);
        let last = (chat.viewport.last + LOAD_MARGIN).min(rows.len() - 1);
        let near = |kind: HoleKind| {
            rows[first..=last]
                .iter()
                .any(|row| matches!(row, RowData::Hole(hole) if *hole == kind))
        };
        let due = |since: Option<Instant>| since.is_none_or(|at| at.elapsed() > LOAD_RETRY);
        let session = Session::as_ref(ctx);
        if near(HoleKind::Older) && due(chat.loading_older) {
            chat.loading_older = Some(Instant::now());
            session.send(Command::LoadOlder(channel.clone()));
        }
        if near(HoleKind::Newer) && due(chat.loading_newer) {
            chat.loading_newer = Some(Instant::now());
            session.send(Command::LoadNewer(channel));
        }
    }

    /// Rebuilds the rows of the chat on screen from the store.
    fn refresh(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            ctx.notify();
            return;
        };
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else {
            return;
        };
        let team = channel_team(&store, &channel);
        if self.lookup_dirty {
            self.lookup = Rc::new(ChatLookup::new(&store, &channel, team.as_ref()));
            *self.docs.borrow_mut() = Docs::default();
            self.lookup_dirty = false;
        }
        self.title = store
            .channel_name(&channel)
            .unwrap_or_else(|| channel.to_string());
        self.icon = Some(icon_kind(&store, &channel));
        // The header as one line of plain text (Markdown removed).
        self.header = store
            .channels
            .get(&channel)
            .map(|channel| mm_render::render(&channel.header, &self.lookup.context()).plain_text())
            .unwrap_or_default();
        self.members = store
            .channel_stats
            .get(&channel)
            .map(|stats| stats.member_count);

        let Some(chat) = self.chats.get(&channel) else {
            return;
        };
        let mut rows = timeline::rows(&store, &channel, chat.last_viewed_at);
        if let Some(filter) = self.active_filter() {
            rows = filtered(rows, &filter.ids);
        }
        let clock = Clock::of(&store);
        let today = clock.today(now_ms());
        let crt = store.crt_enabled();
        let mut wanted: Vec<ImageKey> = Vec::new();
        let mut custom_emoji = CustomEmoji::new();
        let mut data = Vec::with_capacity(rows.len());
        let mut keys = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let key = row.key(rows.get(index + 1));
            let mut hasher = DefaultHasher::new();
            self.theme_generation.hash(&mut hasher);
            let row_data = match row {
                Row::Start => {
                    let (title, subtitle) = start_text(&store, &channel, &self.title);
                    title.hash(&mut hasher);
                    subtitle.hash(&mut hasher);
                    RowData::Start { title, subtitle }
                }
                Row::Date(date) => RowData::Date(timeline::date_label(*date, today)),
                Row::NewMessages => RowData::NewMessages,
                Row::Hole(kind) => RowData::Hole(*kind),
                Row::Activity {
                    text, create_at, ..
                } => {
                    text.hash(&mut hasher);
                    RowData::Activity {
                        text: text.clone(),
                        time: clock.time(*create_at),
                    }
                }
                Row::Post {
                    post,
                    consecutive,
                    pending,
                } => {
                    let author = timeline::author(&store, post);
                    if let Some(picture) = &author.picture {
                        wanted.push(picture.clone());
                    }
                    let reacted: Vec<String> = post
                        .metadata
                        .iter()
                        .flat_map(|metadata| &metadata.reactions)
                        .map(|reaction| reaction.emoji_name.clone())
                        .collect();
                    let names = custom_emoji_names(&post.message)
                        .into_iter()
                        .chain(reacted.iter().map(String::as_str));
                    for name in names {
                        if let Some(emoji) = store.emoji.get(name) {
                            let key = ImageKey::Emoji(emoji.id.clone());
                            if let Some(dir) = &self.account_dir
                                && self.images.contains(&key)
                            {
                                custom_emoji.insert(name.to_owned(), key.path(dir));
                            }
                            wanted.push(key);
                        }
                    }
                    let post_data = PostData {
                        avatar: author
                            .picture
                            .as_ref()
                            .filter(|key| self.images.contains(key))
                            .and_then(|key| Some(key.path(self.account_dir.as_ref()?))),
                        time: clock.time(post.create_at),
                        reactions: reactions(&store, post),
                        files: post
                            .metadata
                            .as_ref()
                            .map(|metadata| {
                                metadata
                                    .files
                                    .iter()
                                    .map(|file| file.name.clone())
                                    .collect()
                            })
                            .unwrap_or_default(),
                        replies: if crt && post.is_root() {
                            post.reply_count
                        } else {
                            0
                        },
                        is_reply: !crt && !post.is_root(),
                        post: post.clone(),
                        consecutive: *consecutive,
                        pending: *pending,
                        author,
                    };
                    (
                        &post_data.post.update_at,
                        post_data.consecutive,
                        post_data.pending.map(|pending| pending == Pending::Failed),
                        &post_data.author.name,
                        &post_data.reactions,
                        &post_data.files,
                        post_data.replies,
                        post_data.is_reply,
                    )
                        .hash(&mut hasher);
                    RowData::Post(Box::new(post_data))
                }
            };
            keys.push((key, hasher.finish()));
            data.push(row_data);
        }
        drop(store);
        self.request_images(wanted, ctx);
        self.custom_emoji = Rc::new(custom_emoji);

        let chat = self.chats.get_mut(&channel).expect("chat");
        chat.list.set_rows(keys);
        if chat.jump_to_new {
            let has_line = data.iter().any(|row| matches!(row, RowData::NewMessages));
            if has_line {
                chat.list
                    .scroll_to(timeline::RowKey::NewMessages, NEW_LINE_OFFSET);
                chat.jump_to_new = false;
            } else if !data.is_empty() && chat.last_viewed_at.is_some() {
                chat.jump_to_new = false;
            }
        }
        self.shared = Rc::new(Shared {
            rows: data,
            lookup: self.lookup.clone(),
            docs: self.docs.clone(),
            custom_emoji: self.custom_emoji.clone(),
        });
        self.load_if_near_an_end(ctx);
        ctx.notify();
    }

    fn request_images(&mut self, keys: Vec<ImageKey>, ctx: &mut ViewContext<Self>) {
        let Some(dir) = self.account_dir.clone() else {
            return;
        };
        for key in keys {
            if self.images.contains(&key) || !self.requested.insert(key.clone()) {
                continue;
            }
            if key.path(&dir).is_file() {
                self.images.insert(key);
            } else {
                Session::as_ref(ctx).send(Command::FetchImage(key));
            }
        }
    }

    fn render_header(&self, chat_colors: &ChatColors, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text = theme.main_text_color(theme.background()).into_solid();
        let muted = theme.sub_text_color(theme.background()).into_solid();
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.);
        if let Some(kind) = self.icon {
            row.add_child(channel_icon(kind, text, chat_colors, font));
        }
        row.add_child(
            Text::new_inline(self.title.clone(), font, size + 2.)
                .with_color(text)
                .with_style(Properties::default().weight(Weight::Bold))
                .finish(),
        );
        row.add_child(
            Expanded::new(
                1.,
                Text::new_inline(self.header.clone(), font, size)
                    .with_color(muted)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish(),
        );
        if let Some(members) = self.members {
            row.add_child(
                Flex::row()
                    .with_spacing(4.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(icon(Icon::Users, muted, 14.))
                    .with_child(
                        Text::new_inline(members.to_string(), font, size)
                            .with_color(muted)
                            .finish(),
                    )
                    .finish(),
            );
        }
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_horizontal_padding(SIDE_PADDING)
                .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
                .finish(),
        )
        .with_height(HEADER_HEIGHT)
        .finish()
    }
}

impl ChannelView {
    /// "carol is typing..." between the timeline and the composer; the line keeps its height
    /// so the composer doesn't move.
    fn render_typing(&self, style: &RowStyle, app: &AppContext) -> Box<dyn Element> {
        let session = Session::as_ref(app);
        let text = self
            .channel
            .as_ref()
            .zip(session.active())
            .and_then(|(channel, active)| {
                let users = active.typing(channel, &PostId::default());
                let store = session.store()?.read().ok()?;
                let names: Vec<String> = users
                    .iter()
                    .map(|user| {
                        store
                            .user_name(user)
                            .unwrap_or_else(|| "Someone".to_owned())
                    })
                    .collect();
                composer::typing_text(&names)
            })
            .unwrap_or_default();
        ConstrainedBox::new(
            Container::new(
                Text::new_inline(text, style.font, style.size - 2.)
                    .with_color(style.muted)
                    .finish(),
            )
            .with_horizontal_padding(SIDE_PADDING + 2.)
            .finish(),
        )
        .with_height(style.size + 6.)
        .finish()
    }
}

/// The rows of a filtered chat: the matching posts (each with its header, since its
/// neighbours may be gone) and the date separators of their days.
fn filtered(rows: Vec<Row>, ids: &HashSet<PostId>) -> Vec<Row> {
    let mut kept: Vec<Row> = Vec::new();
    for row in rows {
        match row {
            Row::Post { post, pending, .. } if ids.contains(&post.id) => kept.push(Row::Post {
                post,
                consecutive: false,
                pending,
            }),
            Row::Date(date) => {
                if matches!(kept.last(), Some(Row::Date(_))) {
                    kept.pop();
                }
                kept.push(Row::Date(date));
            }
            _ => {}
        }
    }
    if matches!(kept.last(), Some(Row::Date(_))) {
        kept.pop();
    }
    kept
}

/// "Showing 12 of 340 messages matching "deploy" · Show all".
fn filter_banner(filter: &ChatFilter, style: &RowStyle) -> Box<dyn Element> {
    let text = format!(
        "Showing {} of {} messages matching \u{201c}{}\u{201d} · ",
        filter.ids.len(),
        filter.total,
        filter.query
    );
    let start = text.chars().count();
    let label = format!("{text}Show all");
    let end = label.chars().count();
    Container::new(
        Text::new_inline(label, style.font, style.size - 1.)
            .with_color(style.text)
            .with_highlights([HighlightedRange {
                highlight: Highlight::new().with_foreground_color(style.palette.link),
                highlight_indices: (start..end).collect(),
            }])
            .with_clickable_char_range(start..end, |_, ctx, _| {
                ctx.dispatch_typed_action(ChannelAction::ClearFilter)
            })
            .finish(),
    )
    .with_horizontal_padding(SIDE_PADDING)
    .with_vertical_padding(8.)
    .with_background_color(fade(style.chat.button_bg, 0.1))
    .finish()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// The team whose channels `~name` refers to: the channel's, else (DMs and GMs) the first of
/// my teams by name.
fn channel_team(store: &Store, channel: &ChannelId) -> Option<TeamId> {
    store
        .channels
        .get(channel)
        .map(|channel| channel.team_id.clone())
        .filter(|team| !team.is_empty())
        .or_else(|| {
            let mut teams: Vec<_> = store.teams.values().collect();
            teams.sort_by(|a, b| a.display_name.cmp(&b.display_name));
            teams.first().map(|team| team.id.clone())
        })
}

fn start_text(store: &Store, channel: &ChannelId, name: &str) -> (String, String) {
    let Some(info) = store.channels.get(channel) else {
        return (format!("Beginning of {name}"), String::new());
    };
    match info.channel_type {
        ChannelType::Direct => {
            let partner = dm_partner(&info.name, &store.me);
            if partner.as_ref() == Some(&store.me) {
                (
                    "This is your space".to_owned(),
                    "Draft messages, list your to-dos or keep links and files handy.".to_owned(),
                )
            } else {
                (
                    name.to_owned(),
                    format!("This is the start of your conversation with {name}."),
                )
            }
        }
        ChannelType::Group => (
            name.to_owned(),
            "This is the start of your conversation with this group.".to_owned(),
        ),
        _ => (format!("Beginning of {name}"), info.purpose.clone()),
    }
}

/// `:name:` tokens of a message (candidates for custom emoji).
fn custom_emoji_names(message: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = message;
    while let Some(start) = rest.find(':') {
        let after = &rest[start + 1..];
        match after.find(':') {
            Some(end)
                if end > 0
                    && after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+')) =>
            {
                names.push(&after[..end]);
                rest = &after[end + 1..];
            }
            Some(_) => rest = after,
            None => break,
        }
    }
    names
}

/// A post's reactions grouped by emoji name, in the order they were first added.
fn reactions(store: &Store, post: &Post) -> Vec<(String, usize, bool)> {
    let Some(metadata) = &post.metadata else {
        return Vec::new();
    };
    let mut grouped: Vec<(String, usize, bool)> = Vec::new();
    let mut sorted: Vec<_> = metadata.reactions.iter().collect();
    sorted.sort_by_key(|reaction| reaction.create_at);
    for reaction in sorted {
        let mine = reaction.user_id == store.me;
        match grouped
            .iter_mut()
            .find(|(name, _, _)| *name == reaction.emoji_name)
        {
            Some(entry) => {
                entry.1 += 1;
                entry.2 |= mine;
            }
            None => grouped.push((reaction.emoji_name.clone(), 1, mine)),
        }
    }
    grouped.into_iter().collect()
}

/// Fonts and colours of the rows, from the theme.
struct RowStyle {
    palette: Palette,
    chat: ChatColors,
    font: FamilyId,
    size: f32,
    text: ColorU,
    muted: ColorU,
    border: ColorU,
    error: ColorU,
}

fn row_style(appearance: &Appearance) -> RowStyle {
    let chat = appearance.theme().chat();
    let palette = Palette::of(appearance);
    RowStyle {
        font: palette.font,
        size: palette.size,
        text: palette.text,
        muted: palette.muted,
        border: palette.border,
        error: chat.error_text,
        chat,
        palette,
    }
}

fn click_handler() -> OnClick {
    Rc::new(|target, ctx| ctx.dispatch_typed_action(ChannelAction::Click(target)))
}

fn post_doc(shared: &Shared, post: &Post) -> Rc<MessageDoc> {
    let mut docs = shared.docs.borrow_mut();
    if let Some((update_at, doc)) = docs.posts.get(&post.id)
        && *update_at == post.update_at
    {
        return doc.clone();
    }
    let doc = Rc::new(if post.is_deleted() {
        mm_render::render("*(message deleted)*", &shared.lookup.context())
    } else {
        mm_render::render_post(post, &shared.lookup.context())
    });
    docs.posts
        .insert(post.id.clone(), (post.update_at, doc.clone()));
    doc
}

fn activity_doc(shared: &Shared, text: &str) -> Rc<MessageDoc> {
    let mut docs = shared.docs.borrow_mut();
    docs.activity
        .entry(text.to_owned())
        .or_insert_with(|| Rc::new(mm_render::render(text, &shared.lookup.context())))
        .clone()
}

fn render_row(
    shared: &Shared,
    style: &RowStyle,
    index: usize,
    flash: Option<&PostId>,
) -> Box<dyn Element> {
    let Some(row) = shared.rows.get(index) else {
        return Empty::new().finish();
    };
    match row {
        RowData::Start { title, subtitle } => {
            let mut column = Flex::column().with_spacing(4.).with_child(
                Text::new(title.clone(), style.font, style.size + 5.)
                    .with_color(style.text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
            if !subtitle.is_empty() {
                column.add_child(
                    Text::new(subtitle.clone(), style.font, style.size)
                        .with_color(style.muted)
                        .finish(),
                );
            }
            Container::new(column.finish())
                .with_horizontal_padding(SIDE_PADDING)
                .with_padding_top(24.)
                .with_padding_bottom(12.)
                .finish()
        }
        RowData::Date(label) => separator(label, style.muted, style.border, style),
        RowData::NewMessages => separator(
            "New messages",
            style.chat.new_message_separator,
            style.chat.new_message_separator,
            style,
        ),
        RowData::Hole(kind) => {
            let (label, action) = match kind {
                HoleKind::Older => ("Load older messages", Some(ChannelAction::Load(*kind))),
                HoleKind::Newer => ("Load newer messages", Some(ChannelAction::Load(*kind))),
                HoleKind::Gap => ("Loading missing messages…", None),
            };
            let length = label.chars().count();
            let mut text = Text::new_inline(label, style.font, style.size - 1.);
            text = match action {
                Some(action) => text
                    .with_color(style.palette.link)
                    .with_clickable_char_range(0..length, move |_, ctx, _| {
                        ctx.dispatch_typed_action(action.clone())
                    }),
                None => text.with_color(style.muted),
            };
            Container::new(Align::new(text.finish()).finish())
                .with_vertical_padding(10.)
                .finish()
        }
        RowData::Activity { text, time } => {
            let doc = activity_doc(shared, text);
            let renderer = MessageRenderer {
                palette: &style.palette,
                custom_emoji: &shared.custom_emoji,
                on_click: click_handler(),
                muted: true,
            };
            let gutter = Text::new_inline(time.clone(), style.font, style.size - 3.)
                .with_color(fade(style.muted, 0.8))
                .finish();
            post_layout(
                Container::new(gutter).with_padding_top(2.).finish(),
                renderer.render(&doc),
                4.,
            )
        }
        RowData::Post(data) => {
            let post = render_post(shared, style, data);
            if flash == Some(&data.post.id) {
                Container::new(post)
                    .with_background_color(fade(style.chat.mention_highlight_bg, 0.55))
                    .finish()
            } else {
                post
            }
        }
    }
}

/// The gutter (avatar or time) and the content of a post-like row.
fn post_layout(gutter: Box<dyn Element>, content: Box<dyn Element>, top: f32) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(fixed_width(GUTTER, gutter))
            .with_child(Shrinkable::new(1., content).finish())
            .finish(),
    )
    .with_horizontal_padding(SIDE_PADDING)
    .with_padding_top(top)
    .with_padding_bottom(2.)
    .finish()
}

fn separator(label: &str, color: ColorU, line: ColorU, style: &RowStyle) -> Box<dyn Element> {
    let rule = || {
        Expanded::new(
            1.,
            ConstrainedBox::new(Rect::new().with_background_color(line).finish())
                .with_height(1.)
                .finish(),
        )
        .finish()
    };
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(rule())
            .with_child(
                Container::new(
                    Text::new_inline(label.to_owned(), style.font, style.size - 2.)
                        .with_color(color)
                        .with_style(Properties::default().weight(Weight::Bold))
                        .finish(),
                )
                .with_horizontal_padding(10.)
                .finish(),
            )
            .with_child(rule())
            .finish(),
    )
    .with_horizontal_padding(SIDE_PADDING)
    .with_vertical_padding(8.)
    .finish()
}

fn render_post(shared: &Shared, style: &RowStyle, data: &PostData) -> Box<dyn Element> {
    let post = &data.post;
    let small = style.size - 2.;
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.);
    if !data.consecutive {
        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Text::new_inline(data.author.name.clone(), style.font, style.size)
                    .with_color(style.text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
        if data.author.bot {
            header.add_child(
                Container::new(
                    Text::new_inline("BOT", style.font, small - 2.)
                        .with_color(style.muted)
                        .with_style(Properties::default().weight(Weight::Bold))
                        .finish(),
                )
                .with_horizontal_padding(4.)
                .with_background_color(fade(style.text, 0.08))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
                .finish(),
            );
        }
        header.add_child(
            Text::new_inline(data.time.clone(), style.font, small)
                .with_color(style.muted)
                .finish(),
        );
        if data.is_reply {
            header.add_child(
                Text::new_inline("replied in a thread", style.font, small)
                    .with_color(style.muted)
                    .finish(),
            );
        }
        content.add_child(header.finish());
    }
    let doc = post_doc(shared, post);
    let renderer = MessageRenderer {
        palette: &style.palette,
        custom_emoji: &shared.custom_emoji,
        on_click: click_handler(),
        muted: post.is_system() || data.pending == Some(Pending::Sending),
    };
    content.add_child(renderer.render(&doc));
    if post.edit_at != 0 && !post.is_deleted() {
        content.add_child(
            Text::new_inline("(edited)", style.font, small)
                .with_color(style.muted)
                .finish(),
        );
    }
    for file in &data.files {
        content.add_child(
            Container::new(
                Flex::row()
                    .with_spacing(6.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(icon(Icon::Paperclip, style.muted, small))
                    .with_child(
                        Text::new_inline(file.clone(), style.font, small)
                            .with_color(style.text)
                            .finish(),
                    )
                    .finish(),
            )
            .with_horizontal_padding(8.)
            .with_vertical_padding(4.)
            .with_border(Border::all(1.).with_border_color(style.border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish(),
        );
    }
    if !data.reactions.is_empty() {
        let mut chips = Wrap::row().with_spacing(4.).with_run_spacing(4.);
        for (name, count, mine) in &data.reactions {
            let emoji = match mm_emoji::by_name(name) {
                Some(emoji) => EmojiRef::System {
                    name: name.clone(),
                    image: emoji.image(),
                },
                None => EmojiRef::Custom { name: name.clone() },
            };
            let side = (small * 1.3).round();
            let image = emoji_image(&emoji, &shared.custom_emoji)
                .map(|image| {
                    ConstrainedBox::new(image)
                        .with_width(side)
                        .with_height(side)
                        .finish()
                })
                .unwrap_or_else(|| {
                    Text::new_inline(format!(":{name}:"), style.font, small)
                        .with_color(style.text)
                        .finish()
                });
            let mut chip = Container::new(
                Flex::row()
                    .with_spacing(4.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(image)
                    .with_child(
                        Text::new_inline(count.to_string(), style.font, small)
                            .with_color(if *mine { style.chat.link } else { style.text })
                            .finish(),
                    )
                    .finish(),
            )
            .with_horizontal_padding(6.)
            .with_vertical_padding(2.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
            chip = if *mine {
                chip.with_background_color(fade(style.chat.link, 0.12))
                    .with_border(Border::all(1.).with_border_color(style.chat.link))
            } else {
                chip.with_background_color(fade(style.text, 0.06))
            };
            chips.add_child(chip.finish());
        }
        content.add_child(chips.finish());
    }
    if data.replies > 0 {
        let label = if data.replies == 1 {
            "1 reply".to_owned()
        } else {
            format!("{} replies", data.replies)
        };
        content.add_child(
            Flex::row()
                .with_spacing(6.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(icon(Icon::MessageChatSquare, style.palette.link, small))
                .with_child(
                    Text::new_inline(label, style.font, small)
                        .with_color(style.palette.link)
                        .finish(),
                )
                .finish(),
        );
    }
    match data.pending {
        Some(Pending::Sending) => content.add_child(
            Text::new_inline("Sending…", style.font, small)
                .with_color(style.muted)
                .finish(),
        ),
        Some(Pending::Failed) => content.add_child(failed_line(post.id.to_string(), style)),
        None => {}
    }
    let gutter = if data.consecutive {
        Empty::new().finish()
    } else {
        avatar(&data.author, data.avatar.as_ref(), style)
    };
    post_layout(
        gutter,
        content.finish(),
        if data.consecutive { 2. } else { 10. },
    )
}

/// "Message not sent. Retry · Delete" with the two links.
fn failed_line(pending: String, style: &RowStyle) -> Box<dyn Element> {
    const LABEL: &str = "Message not sent. Retry · Delete";
    let link = |word: &str| {
        let start = LABEL[..LABEL.find(word).unwrap_or(0)].chars().count();
        start..start + word.chars().count()
    };
    let (retry, delete) = (link("Retry"), link("Delete"));
    let highlight = |range: &std::ops::Range<usize>| HighlightedRange {
        highlight: Highlight::new().with_foreground_color(style.palette.link),
        highlight_indices: range.clone().collect(),
    };
    let retry_id = pending.clone();
    Text::new_inline(LABEL, style.font, style.size - 2.)
        .with_color(style.error)
        .with_highlights([highlight(&retry), highlight(&delete)])
        .with_clickable_char_range(retry, move |_, ctx, _| {
            ctx.dispatch_typed_action(ChannelAction::Retry(retry_id.clone()))
        })
        .with_clickable_char_range(delete, move |_, ctx, _| {
            ctx.dispatch_typed_action(ChannelAction::Discard(pending.clone()))
        })
        .finish()
}

fn avatar(author: &Author, path: Option<&PathBuf>, style: &RowStyle) -> Box<dyn Element> {
    let round = CornerRadius::with_all(Radius::Percentage(50.));
    let element = match path {
        Some(path) => Image::new(
            AssetSource::LocalFile {
                path: path.to_string_lossy().into_owned(),
                content_version: None,
            },
            CacheOption::BySize,
        )
        .with_corner_radius(round)
        .finish(),
        None => Container::new(
            Align::new(
                Text::new_inline(author.initials.clone(), style.font, style.size - 2.)
                    .with_color(style.chat.button_text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(fade(style.chat.button_bg, 0.85))
        .with_corner_radius(round)
        .finish(),
    };
    ConstrainedBox::new(element)
        .with_width(AVATAR)
        .with_height(AVATAR)
        .finish()
}

impl Entity for ChannelView {
    type Event = ChannelEvent;
}

impl View for ChannelView {
    fn ui_name() -> &'static str {
        "ChannelView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            if std::mem::take(&mut self.focus_timeline) {
                return;
            }
            ctx.focus(&self.composer);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let Some(chat) = self
            .channel
            .as_ref()
            .and_then(|channel| self.chats.get(channel))
        else {
            return Empty::new().finish();
        };
        let style = Rc::new(row_style(appearance));
        let header = self.render_header(&style.chat, appearance);
        let shared = self.shared.clone();
        let row_style = style.clone();
        let flash = self.flash.clone();
        let list = TimelineList::new(chat.list.clone(), move |index, _| {
            render_row(&shared, &row_style, index, flash.as_ref())
        });
        let timeline = Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            SCROLLBAR,
            fade(style.text, 0.25).into(),
            fade(style.text, 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        let timeline = Container::new(timeline).with_padding_bottom(10.).finish();
        let timeline = match self.active_filter() {
            Some(filter) => Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(filter_banner(filter, &style))
                .with_child(Shrinkable::new(1., timeline).finish())
                .finish(),
            None => timeline,
        };
        let mut stack = Stack::new().with_child(timeline);
        if !chat.viewport.at_bottom && !self.shared.rows.is_empty() {
            let chat_colors = style.chat;
            let font = style.font;
            let size = style.size;
            let button = Hoverable::new(self.jump_mouse_state.clone(), move |state| {
                let background = if state.is_hovered() {
                    chat_colors.button_bg
                } else {
                    fade(chat_colors.button_bg, 0.9)
                };
                Container::new(
                    Text::new_inline("Jump to newest ↓", font, size - 1.)
                        .with_color(chat_colors.button_text)
                        .with_style(Properties::default().weight(Weight::Bold))
                        .finish(),
                )
                .with_horizontal_padding(12.)
                .with_vertical_padding(6.)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(ChannelAction::JumpToNewest))
            .finish();
            stack.add_child(
                Container::new(Align::new(button).bottom_center().finish())
                    .with_padding_bottom(12.)
                    .finish(),
            );
        }
        let column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(Shrinkable::new(1., stack.finish()).finish())
            .with_child(self.render_typing(&style, app))
            .with_child(
                Container::new(ChildView::new(&self.composer).finish())
                    .with_horizontal_padding(SIDE_PADDING)
                    .with_padding_bottom(14.)
                    .finish(),
            )
            .finish();
        EventHandler::new(column)
            .on_left_mouse_down(|ctx, _, _| {
                ctx.dispatch_typed_action(ChannelAction::Focus);
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }
}

impl TypedActionView for ChannelView {
    type Action = ChannelAction;

    fn handle_action(&mut self, action: &ChannelAction, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        match action {
            ChannelAction::Click(Target::Url(url)) => ctx.open_url(url),
            ChannelAction::Click(Target::Channel(name)) => {
                if let Some(id) = self.lookup.channel_id(name) {
                    ctx.emit(ChannelEvent::OpenChannel(id.clone()));
                }
            }
            // Profiles, DMs and search come with later steps.
            ChannelAction::Click(Target::Mention(_) | Target::Hashtag(_)) => {}
            ChannelAction::Load(kind) => {
                let command = match kind {
                    HoleKind::Older => Command::LoadOlder(channel),
                    HoleKind::Newer => Command::LoadNewer(channel),
                    HoleKind::Gap => return,
                };
                Session::as_ref(ctx).send(command);
            }
            ChannelAction::JumpToNewest => {
                if let Some(chat) = self.chats.get(&channel) {
                    chat.list.scroll_to_bottom();
                }
                // Opened at the first unread post: the newest page may not be loaded yet.
                Session::as_ref(ctx).send(Command::LoadNewer(channel));
                ctx.notify();
            }
            ChannelAction::MarkRead => Session::as_ref(ctx).send(Command::MarkRead(channel)),
            ChannelAction::ClearFilter => ctx.emit(ChannelEvent::ClearFilter),
            ChannelAction::Focus => {
                self.focus_timeline = true;
                ctx.focus_self();
            }
            ChannelAction::Retry(pending) => {
                Session::as_ref(ctx).send(Command::RetryPost(pending.clone()))
            }
            ChannelAction::Discard(pending) => {
                Session::as_ref(ctx).send(Command::DiscardPost(pending.clone()))
            }
            ChannelAction::PageUp | ChannelAction::PageDown => {
                if let Some(chat) = self.chats.get(&channel) {
                    chat.list.scroll_page(*action == ChannelAction::PageUp);
                }
                ctx.notify();
            }
        }
    }
}
