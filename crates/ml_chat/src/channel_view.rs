// SPDX-License-Identifier: AGPL-3.0-only

//! A chat's content (MatterLite.md §10.2, §10.4): a channel in a tab, or a reply thread in
//! the right panel (the root post, its replies and a composer that replies), with the same
//! rows and post actions. For a channel: the channel header and the timeline,
//! anchored at the bottom, with date separators, the "New messages" line, combined
//! membership messages, posts with avatars, holes that load older or newer posts when they
//! come near, pending and failed posts with Retry and Delete, and "Jump to newest". A post
//! is edited in place: an editor takes the message's place until it is saved or cancelled.
//! Reaction chips add or remove mine; React and the `+` chip open the emoji picker.
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

use ml_editor::{EditorView, Event as EditorEvent};
use ml_themes::ChatColors;
use mm_model::{ChannelId, ChannelType, Post, PostId, TeamId};
use mm_render::MessageDoc;
use mm_store::{PostActions, Store, StoreDelta, dm_partner};
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
use warpui::ui_components::components::UiComponent as _;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats::{ChannelIcon, channel_icon as icon_kind};
use crate::chats_view::{channel_icon, fade, icon};
use crate::composer::{self, EnterKey, EnterPrefs, OnEnter};
use crate::composer_view::{ComposerEvent, ComposerView, editor_options};
use crate::emoji_picker::{EmojiPickerView, PickerEvent};
use crate::history_pane::ChatFilter;
use crate::message::{
    CustomEmoji, MessageRenderer, OnClick, Palette, Target, emoji_image, fixed_width,
};
use ml_workspace::icon_button_with_color;

use crate::post_menu::{
    PickerAnchor, PostAction, PostMenu, PostMenuHost, PostMenuItem, ToolbarState, ToolbarStates,
    render_toolbar,
};
use crate::session::{Session, SessionEvent};
use crate::tabs::ChatKey;
use crate::timeline::{self, Author, ChatLookup, Clock, HoleKind, Pending, ReactionChip, Row};
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
/// The editor of a post being edited grows with its text up to this height, then scrolls.
const MAX_EDIT_HEIGHT: f32 = 320.;

/// Where the view is: a chat tab, or the right panel (a thread, with a close button; Esc
/// closes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatMode {
    Tab,
    Panel,
}

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
    /// The post toolbar and menu.
    Post(PostAction),
    /// The thread header's Follow/Unfollow.
    ToggleFollow,
    /// The right panel's close button.
    Close,
    /// The right panel's "Open in tab".
    OpenInTab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEvent {
    /// A `~channel` link was clicked.
    OpenChannel(ChannelId),
    /// "Show all" in the filter banner: turn "Filter the chat" off.
    ClearFilter,
    /// Delete in the post menu: ask before deleting (a root post takes its replies).
    ConfirmDelete { post: PostId, replies: i64 },
    /// A new emoji on a post with `UniqueEmojiReactionLimitPerPost` distinct ones already.
    ReactionLimit,
    /// Reply on a post of a channel: show its thread (in the right panel).
    OpenThread(PostId),
    /// The right panel's thread view closes (its close button or Esc).
    Close,
    /// "Open in tab": the right panel's thread moves to a chat tab.
    OpenInTab(PostId),
    /// "(edited)" on my post: show its edit history in the right panel.
    ShowEditHistory(PostId),
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
    ReplyCount(usize),
}

struct PostData {
    post: Arc<Post>,
    consecutive: bool,
    pending: Option<Pending>,
    author: Author,
    avatar: Option<PathBuf>,
    time: String,
    reactions: Vec<ReactionChip>,
    /// "(edited)": its tooltip, and whether a click shows the edit history (my post, while
    /// I may edit it, as the webapp's `PostEditedIndicator`).
    edited: Option<(String, bool)>,
    files: Vec<String>,
    /// Replies of a root post under Collapsed Reply Threads.
    replies: i64,
    /// A reply shown in the channel (CRT off).
    is_reply: bool,
    /// What the post toolbar and menu offer.
    actions: PostActions,
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
    /// Hover states of the posts (kept across refreshes) and of the toolbar's buttons.
    hover: Rc<RefCell<HashMap<PostId, MouseStateHandle>>>,
    toolbar: Rc<ToolbarStates>,
    /// Hover states of the reaction chips, by post and emoji (`+` for the add chip).
    chips: Rc<RefCell<HashMap<(PostId, String), MouseStateHandle>>>,
    /// The saved positions and action wrapper of this view's post toolbar and menu.
    posts: PostMenuHost<ChannelAction>,
}

/// What rows draw differently from one frame to the next.
struct RowState {
    /// A post shown from elsewhere, highlighted for a moment.
    flash: Option<PostId>,
    /// The post whose menu or emoji picker is open: it keeps its toolbar.
    menu: Option<PostId>,
    picker: Option<PostId>,
    /// A thread is shown: posts have no Reply button (the composer replies).
    thread: bool,
    /// The post being edited, and the editor that takes its message's place.
    editing: Option<Editing>,
    editor: ViewHandle<EditorView>,
}

/// A post being edited in place (the webapp's inline edit, `components/edit_post`).
#[derive(Clone)]
struct Editing {
    post: PostId,
    /// The message when editing started: saving it unchanged only closes the editor.
    original: String,
    /// "Enter to save · Esc to cancel", per the Enter preference.
    hint: &'static str,
    /// The text is too long to save.
    error: Option<String>,
}

pub struct ChannelView {
    mode: ChatMode,
    posts: PostMenuHost<ChannelAction>,
    /// What is shown, and its channel (a thread's, once its root post is known).
    key: Option<ChatKey>,
    channel: Option<ChannelId>,
    chats: HashMap<ChatKey, Chat>,
    /// A thread: whether I follow it (CRT), for the header's button.
    following: Option<bool>,
    /// The thread last marked read, and its last reply then.
    thread_read: Option<(PostId, i64)>,
    follow_state: MouseStateHandle,
    open_in_tab_state: MouseStateHandle,
    close_state: MouseStateHandle,
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
    /// A post to show once it is loaded (`LoadAround` was asked for).
    pending_show: Option<PostId>,
    /// The next focus of this view stays on the timeline (a click in it) instead of moving
    /// on to the composer.
    focus_timeline: bool,
    hover: Rc<RefCell<HashMap<PostId, MouseStateHandle>>>,
    toolbar: Rc<ToolbarStates>,
    menu: Option<PostMenu>,
    /// The editor of the post being edited (one per view; one post is edited at a time).
    edit_editor: ViewHandle<EditorView>,
    editing: Option<Editing>,
    chips: Rc<RefCell<HashMap<(PostId, String), MouseStateHandle>>>,
    /// The emoji picker, and the post it is open for (with where it is).
    picker: ViewHandle<EmojiPickerView>,
    picking: Option<(PostId, PickerAnchor, bool)>,
}

impl ChannelView {
    /// The view of the chat tabs.
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        Self::with_mode(ChatMode::Tab, ctx)
    }

    /// The thread view of the right panel.
    pub fn new_panel(ctx: &mut ViewContext<Self>) -> Self {
        Self::with_mode(ChatMode::Panel, ctx)
    }

    fn with_mode(mode: ChatMode, ctx: &mut ViewContext<Self>) -> Self {
        let posts = PostMenuHost {
            prefix: match mode {
                ChatMode::Tab => "ml_chat:channel",
                ChatMode::Panel => "ml_chat:thread",
            },
            wrap: ChannelAction::Post,
        };
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.theme_generation += 1;
            for chat in me.chats.values() {
                chat.list.clear_heights();
            }
            let appearance = Appearance::as_ref(ctx);
            let (font, size) = (appearance.ui_font_family(), appearance.ui_font_size() + 1.);
            me.edit_editor.update(ctx, |editor, ctx| {
                editor.set_font_family(font, ctx);
                editor.set_font_size(size, ctx);
            });
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
                let key = ChatKey::Channel(channel.clone());
                if let Some(chat) = me.chats.get_mut(&key) {
                    chat.last_viewed_at = Some(*last_viewed_at);
                    chat.jump_to_new = true;
                    if me.key.as_ref() == Some(&key) {
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
                if let Some(chat) = me.chat() {
                    chat.list.scroll_to_bottom();
                }
                ctx.notify();
            }
            ComposerEvent::Escape => me.handle_action(&ChannelAction::MarkRead, ctx),
            ComposerEvent::EditLast => me.edit_last(ctx),
        });
        let options = editor_options(Appearance::as_ref(ctx));
        let edit_editor = ctx.add_typed_action_view(|ctx| EditorView::new(options, ctx));
        ctx.subscribe_to_view(&edit_editor, |me, _, event, ctx| {
            me.on_edit_event(event, ctx)
        });
        let picker = ctx.add_typed_action_view(EmojiPickerView::new);
        ctx.subscribe_to_view(&picker, |me, _, event, ctx| match event {
            PickerEvent::Picked(emoji) => {
                if let Some((post, _, _)) = me.picking.clone() {
                    me.add_reaction(&post, emoji, ctx);
                }
                me.close_picker(true, ctx);
            }
            PickerEvent::Closed => me.close_picker(true, ctx),
        });
        let chips = Rc::new(RefCell::new(HashMap::new()));
        let lookup = Rc::new(ChatLookup::default());
        let docs = Rc::new(RefCell::new(Docs::default()));
        let custom_emoji = Rc::new(CustomEmoji::new());
        let hover = Rc::new(RefCell::new(HashMap::new()));
        let toolbar = Rc::new(ToolbarStates::default());
        Self {
            mode,
            posts,
            key: None,
            channel: None,
            chats: HashMap::new(),
            following: None,
            thread_read: None,
            follow_state: MouseStateHandle::default(),
            open_in_tab_state: MouseStateHandle::default(),
            close_state: MouseStateHandle::default(),
            shared: Rc::new(Shared {
                rows: Vec::new(),
                lookup: lookup.clone(),
                docs: docs.clone(),
                custom_emoji: custom_emoji.clone(),
                hover: hover.clone(),
                toolbar: toolbar.clone(),
                chips: chips.clone(),
                posts,
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
            pending_show: None,
            focus_timeline: false,
            hover,
            toolbar,
            menu: None,
            edit_editor,
            editing: None,
            chips,
            picker,
            picking: None,
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
            .filter(|filter| self.key.as_ref() == Some(&ChatKey::Channel(filter.channel.clone())))
    }

    /// The chat on screen.
    fn chat(&self) -> Option<&Chat> {
        self.key.as_ref().and_then(|key| self.chats.get(key))
    }

    /// The thread on screen.
    fn root(&self) -> Option<&PostId> {
        match &self.key {
            Some(ChatKey::Thread(root)) => Some(root),
            _ => None,
        }
    }

    /// The composer posts to the chat, or replies to the thread.
    fn update_composer(&mut self, ctx: &mut ViewContext<Self>) {
        let channel = self.channel.clone();
        let root = self.root().cloned().unwrap_or_default();
        self.composer
            .update(ctx, |composer, ctx| composer.set_target(channel, root, ctx));
    }

    /// Scrolls the open chat to a post and highlights it for a moment (the webapp's permalink
    /// view). A post that isn't loaded is fetched with the posts around it and shown then;
    /// false when it can't be (no chat, or a thread).
    pub fn show_post(&mut self, post: &PostId, ctx: &mut ViewContext<Self>) -> bool {
        if self.try_show(post, ctx) {
            self.pending_show = None;
            return true;
        }
        let Some(ChatKey::Channel(channel)) = self.key.clone() else {
            return false;
        };
        self.pending_show = Some(post.clone());
        Session::as_ref(ctx).send(Command::LoadAround {
            channel,
            post: post.clone(),
        });
        true
    }

    fn try_show(&mut self, post: &PostId, ctx: &mut ViewContext<Self>) -> bool {
        let key = timeline::RowKey::Post(post.clone());
        let found = self
            .shared
            .rows
            .iter()
            .any(|row| matches!(row, RowData::Post(data) if data.post.id == *post));
        if !found {
            return false;
        }
        let Some(chat) = self.key.clone().and_then(|key| self.chats.get_mut(&key)) else {
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

    /// Shows a thread (the right panel's), or nothing.
    pub fn set_thread(&mut self, root: Option<PostId>, ctx: &mut ViewContext<Self>) {
        self.set_key(root.map(ChatKey::Thread), ctx);
    }

    /// The thread shown, if any.
    pub fn thread(&self) -> Option<&PostId> {
        self.root()
    }

    /// Shows a chat or a thread (the active tab's), or nothing.
    pub fn set_key(&mut self, key: Option<ChatKey>, ctx: &mut ViewContext<Self>) {
        if key == self.key {
            return;
        }
        if let Some(key) = &key
            && !self.chats.contains_key(key)
        {
            let (list, viewports) = TimelineState::new();
            let id = key.clone();
            ctx.spawn_stream_local(
                viewports,
                move |me, viewport, ctx| me.on_viewport(&id, viewport, ctx),
                |_, _| {},
            );
            self.chats.insert(
                key.clone(),
                Chat {
                    list,
                    last_viewed_at: None,
                    // A thread opens at its newest reply.
                    jump_to_new: matches!(key, ChatKey::Channel(_)),
                    viewport: Viewport::default(),
                    loading_older: None,
                    loading_newer: None,
                },
            );
        }
        if self.mode == ChatMode::Panel {
            // The panel shows one thread at a time.
            self.chats.retain(|known, _| Some(known) == key.as_ref());
        }
        self.channel = match &key {
            Some(ChatKey::Channel(channel)) => Some(channel.clone()),
            Some(ChatKey::Thread(root)) => Session::as_ref(ctx)
                .store()
                .and_then(|store| Some(store.read().ok()?.posts.get(root)?.channel_id.clone())),
            None => None,
        };
        if let Some(ChatKey::Thread(root)) = &key {
            Session::as_ref(ctx).send(Command::OpenThread(root.clone()));
        }
        self.key = key;
        self.editing = None;
        self.menu = None;
        self.picking = None;
        self.pending_show = None;
        self.following = None;
        self.thread_read = None;
        self.lookup_dirty = true;
        self.update_composer(ctx);
        self.refresh(ctx);
    }

    /// Forgets the chats and threads whose tabs closed.
    pub fn retain(&mut self, open: &[ChatKey]) {
        self.chats.retain(|key, _| open.contains(key));
    }

    fn on_changes(&mut self, deltas: &[StoreDelta], ctx: &mut ViewContext<Self>) {
        let Some(key) = self.key.clone() else {
            return;
        };
        // The channel's own timeline and member (a thread's channel matters for its name).
        let channel = match &key {
            ChatKey::Channel(channel) => channel.clone(),
            ChatKey::Thread(_) => self.channel.clone().unwrap_or_default(),
        };
        let is_channel = matches!(key, ChatKey::Channel(_));
        let mut relevant = false;
        for delta in deltas {
            match delta {
                StoreDelta::TimelineChanged(changed) => {
                    if *changed == channel && is_channel {
                        relevant = true;
                        if let Some(chat) = self.chats.get_mut(&key) {
                            chat.loading_older = None;
                            chat.loading_newer = None;
                        }
                    }
                }
                StoreDelta::ThreadChanged(root) => {
                    if key == ChatKey::Thread(root.clone()) {
                        relevant = true;
                    }
                }
                StoreDelta::PostsChanged(_)
                | StoreDelta::PostsRemoved(_)
                | StoreDelta::PendingChanged(_) => relevant = true,
                StoreDelta::RolesChanged => {
                    self.composer
                        .update(ctx, |composer, ctx| composer.update_state(ctx));
                }
                StoreDelta::MemberChanged(changed) if *changed == channel && is_channel => {
                    relevant = true;
                    self.lookup_dirty = true;
                    // Marked unread by hand (also elsewhere): the "New messages" line moves to
                    // the first unread post.
                    let last_viewed = Session::as_ref(ctx).store().and_then(|store| {
                        let store = store.read().ok()?;
                        store
                            .manually_unread
                            .contains(&channel)
                            .then(|| store.channel_members.get(&channel))
                            .flatten()
                            .map(|member| member.last_viewed_at)
                    });
                    if let Some(last_viewed) = last_viewed
                        && let Some(chat) = self.chats.get_mut(&key)
                    {
                        chat.last_viewed_at = Some(last_viewed);
                    }
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

    fn on_viewport(&mut self, key: &ChatKey, viewport: Viewport, ctx: &mut ViewContext<Self>) {
        let at_bottom_changed = self
            .chats
            .get(key)
            .is_some_and(|chat| chat.viewport.at_bottom != viewport.at_bottom);
        if let Some(chat) = self.chats.get_mut(key) {
            chat.viewport = viewport;
        }
        if self.key.as_ref() == Some(key) {
            self.load_if_near_an_end(ctx);
            if at_bottom_changed {
                ctx.notify();
            }
        }
    }

    /// Asks for the next page when a loadable hole is within a few rows of the screen.
    fn load_if_near_an_end(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(ChatKey::Channel(channel)) = self.key.clone() else {
            return;
        };
        let rows = &self.shared.rows;
        let Some(chat) = self.chats.get_mut(&ChatKey::Channel(channel.clone())) else {
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
        let Some(key) = self.key.clone() else {
            ctx.notify();
            return;
        };
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else {
            return;
        };
        // A thread's channel is known once its root post is.
        let channel = match &key {
            ChatKey::Channel(channel) => Some(channel.clone()),
            ChatKey::Thread(root) => store.posts.get(root).map(|post| post.channel_id.clone()),
        };
        let channel_changed = channel != self.channel;
        if channel_changed {
            self.channel = channel.clone();
            self.lookup_dirty = true;
        }
        if let Some(channel) = &channel {
            let team = channel_team(&store, channel);
            if self.lookup_dirty {
                self.lookup = Rc::new(ChatLookup::new(&store, channel, team.as_ref()));
                *self.docs.borrow_mut() = Docs::default();
                self.lookup_dirty = false;
            }
            self.title = store
                .channel_name(channel)
                .unwrap_or_else(|| channel.to_string());
            self.icon = Some(icon_kind(&store, channel));
            // The header as one line of plain text (Markdown removed).
            self.header = store
                .channels
                .get(channel)
                .map(|channel| {
                    mm_render::render(&channel.header, &self.lookup.context()).plain_text()
                })
                .unwrap_or_default();
            self.members = store
                .channel_stats
                .get(channel)
                .map(|stats| stats.member_count);
        }
        // A thread: Follow in the header, and marking it read while it is on screen (CRT,
        // `markThreadAsRead` when the thread has unread replies).
        let mut mark_read = None;
        if let ChatKey::Thread(root) = &key {
            let actions = store.post_actions(root, now_ms());
            self.following = actions.follow.then_some(actions.following);
            if store.crt_enabled()
                && let Some(state) = store.thread_states.get(root)
                && (state.unread_replies > 0 || state.unread_mentions > 0)
                && self.thread_read != Some((root.clone(), state.last_reply_at))
            {
                self.thread_read = Some((root.clone(), state.last_reply_at));
                mark_read = Some(root.clone());
            }
        }

        let Some(chat) = self.chats.get(&key) else {
            return;
        };
        let mut rows = match &key {
            ChatKey::Channel(channel) => timeline::rows(&store, channel, chat.last_viewed_at),
            ChatKey::Thread(root) => timeline::thread_rows(&store, root),
        };
        let in_thread = matches!(key, ChatKey::Thread(_));
        if let Some(filter) = self.active_filter() {
            rows = filtered(rows, &filter.ids);
        }
        let clock = Clock::of(&store);
        let now = now_ms();
        let today = clock.today(now);
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
                    let (title, subtitle) = channel
                        .as_ref()
                        .map(|channel| start_text(&store, channel, &self.title))
                        .unwrap_or_default();
                    title.hash(&mut hasher);
                    subtitle.hash(&mut hasher);
                    RowData::Start { title, subtitle }
                }
                Row::Date(date) => RowData::Date(timeline::date_label(*date, today)),
                Row::NewMessages => RowData::NewMessages,
                Row::Hole(kind) => RowData::Hole(*kind),
                Row::ReplyCount(count) => {
                    count.hash(&mut hasher);
                    RowData::ReplyCount(*count)
                }
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
                        reactions: {
                            let actions = store.post_actions(&post.id, now);
                            timeline::reaction_chips(&store, post, actions.react, actions.unreact)
                        },
                        edited: (post.edit_at != 0 && !post.is_deleted()).then(|| {
                            let history =
                                post.user_id == store.me && store.post_actions(&post.id, now).edit;
                            let mut label = timeline::edited_label(&clock, post.edit_at, now);
                            if history {
                                label.push_str(" · Click to view history");
                            }
                            (label, history)
                        }),
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
                        replies: if crt && post.is_root() && !in_thread {
                            post.reply_count
                        } else {
                            0
                        },
                        is_reply: !crt && !post.is_root() && !in_thread,
                        actions: store.post_actions(&post.id, now),
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
                        post_data.post.is_pinned,
                        post_data.actions.saved,
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
        if channel_changed {
            self.update_composer(ctx);
        }
        if let Some(root) = mark_read {
            Session::as_ref(ctx).send(Command::MarkThreadRead(root));
        }

        let chat = self.chats.get_mut(&key).expect("chat");
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
        // Hover states of posts no longer shown go, and so does the edit of a deleted post.
        let shown = |id: &PostId| {
            data.iter()
                .any(|row| matches!(row, RowData::Post(post) if post.post.id == *id))
        };
        self.hover.borrow_mut().retain(|id, _| shown(id));
        self.chips.borrow_mut().retain(|(id, _), _| shown(id));
        if self
            .picking
            .as_ref()
            .is_some_and(|(post, _, _)| !shown(post))
        {
            self.picking = None;
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|editing| !shown(&editing.post))
        {
            self.stop_edit(ctx);
        }
        self.shared = Rc::new(Shared {
            rows: data,
            lookup: self.lookup.clone(),
            docs: self.docs.clone(),
            custom_emoji: self.custom_emoji.clone(),
            hover: self.hover.clone(),
            toolbar: self.toolbar.clone(),
            chips: self.chips.clone(),
            posts: self.posts,
        });
        self.load_if_near_an_end(ctx);
        if let Some(post) = self.pending_show.clone()
            && self.try_show(&post, ctx)
        {
            self.pending_show = None;
        }
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
        if self.root().is_some() {
            return self.render_thread_header(appearance);
        }
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
    /// A thread's header (the webapp's RHS header): "Thread", the channel's name, Follow or
    /// Following (CRT), and in the right panel a close button.
    fn render_thread_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text = theme.main_text_color(theme.background()).into_solid();
        let muted = theme.sub_text_color(theme.background()).into_solid();
        let link = theme.chat().link;
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Text::new_inline("Thread", font, size + 2.)
                    .with_color(text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(self.title.clone(), font, size)
                        .with_color(muted)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            );
        if let Some(following) = self.following {
            let label = if following { "Following" } else { "Follow" };
            row.add_child(
                Hoverable::new(self.follow_state.clone(), move |state| {
                    let mut chip = Container::new(
                        Text::new_inline(label, font, size - 1.)
                            .with_color(link)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .with_horizontal_padding(10.)
                    .with_vertical_padding(4.)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
                    chip = if following {
                        chip.with_background_color(fade(
                            link,
                            if state.is_hovered() { 0.16 } else { 0.08 },
                        ))
                    } else {
                        chip.with_border(Border::all(1.).with_border_color(fade(link, 0.6)))
                            .with_background_color(fade(
                                link,
                                if state.is_hovered() { 0.08 } else { 0. },
                            ))
                    };
                    chip.finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(ChannelAction::ToggleFollow))
                .finish(),
            );
        }
        if self.mode == ChatMode::Panel {
            let tooltip = appearance
                .ui_builder()
                .tool_tip("Open in tab".to_owned())
                .build()
                .finish();
            row.add_child(
                icon_button_with_color(
                    appearance,
                    Icon::LinkExternal,
                    false,
                    self.open_in_tab_state.clone(),
                    theme.sub_text_color(theme.background()),
                )
                .with_tooltip(move || tooltip)
                .build()
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(ChannelAction::OpenInTab))
                .with_cursor(Cursor::PointingHand)
                .finish(),
            );
            let tooltip = appearance
                .ui_builder()
                .tool_tip("Close (Esc)".to_owned())
                .build()
                .finish();
            row.add_child(
                icon_button_with_color(
                    appearance,
                    Icon::X,
                    false,
                    self.close_state.clone(),
                    theme.sub_text_color(theme.background()),
                )
                .with_tooltip(move || tooltip)
                .build()
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(ChannelAction::Close))
                .with_cursor(Cursor::PointingHand)
                .finish(),
            );
        }
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_padding_left(SIDE_PADDING)
                .with_padding_right(8.)
                .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
                .finish(),
        )
        .with_height(HEADER_HEIGHT)
        .finish()
    }

    /// "carol is typing..." between the timeline and the composer; the line keeps its height
    /// so the composer doesn't move.
    fn render_typing(&self, style: &RowStyle, app: &AppContext) -> Box<dyn Element> {
        let session = Session::as_ref(app);
        let text = self
            .channel
            .as_ref()
            .zip(session.active())
            .and_then(|(channel, active)| {
                let users = active.typing(channel, &self.root().cloned().unwrap_or_default());
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
    state: &RowState,
    appearance: &Appearance,
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
        RowData::ReplyCount(count) => separator(
            &timeline::replies_label(*count),
            style.muted,
            style.border,
            style,
        ),
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
                HoleKind::Replies => ("Loading replies…", None),
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
            let id = data.post.id.clone();
            let mouse_state = shared
                .hover
                .borrow_mut()
                .entry(id.clone())
                .or_default()
                .clone();
            let flashing = state.flash.as_ref() == Some(&id);
            let menu_open = state.menu.as_ref() == Some(&id);
            let picker_open = state.picker.as_ref() == Some(&id);
            let editing = state
                .editing
                .as_ref()
                .filter(|editing| editing.post == id)
                .map(|editing| (editing, &state.editor));
            // The edited post has no toolbar or menu.
            let has_actions = data.actions.any() && editing.is_none();
            let menu_id = id.clone();
            let posts = shared.posts;
            Hoverable::new(mouse_state, |mouse| {
                let highlighted = has_actions && (mouse.is_hovered() || menu_open || picker_open);
                let mut post =
                    Container::new(render_post(shared, style, data, editing, appearance));
                if flashing {
                    post = post.with_background_color(fade(style.chat.mention_highlight_bg, 0.55));
                } else if editing.is_some() {
                    post = post.with_background_color(fade(style.chat.button_bg, 0.08));
                } else if highlighted {
                    post = post.with_background_color(fade(style.text, 0.04));
                }
                if !highlighted {
                    return post.finish();
                }
                let mut stack = Stack::new().with_child(post.finish());
                stack.add_positioned_child(
                    render_toolbar(
                        shared.posts,
                        &shared.toolbar,
                        &id,
                        ToolbarState {
                            react: data.actions.emoji_picker,
                            reply: data.actions.reply && !state.thread,
                            menu_open,
                            picker_open,
                        },
                        appearance,
                    ),
                    warpui::elements::OffsetPositioning::offset_from_parent(
                        pathfinder_geometry::vector::vec2f(-SIDE_PADDING, 2.),
                        warpui::elements::ParentOffsetBounds::ParentByPosition,
                        warpui::elements::ParentAnchor::TopRight,
                        warpui::elements::ChildAnchor::TopRight,
                    ),
                );
                stack.finish()
            })
            .on_right_click(move |ctx, _, position| {
                if has_actions {
                    let action = posts.open_at(ctx, menu_id.clone(), position);
                    ctx.dispatch_typed_action(action);
                }
            })
            .finish()
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

fn render_post(
    shared: &Shared,
    style: &RowStyle,
    data: &PostData,
    editing: Option<(&Editing, &ViewHandle<EditorView>)>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let post = &data.post;
    let small = style.size - 2.;
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.);
    // The webapp's post pre-header.
    if post.is_pinned || data.actions.saved {
        let mut labels = Flex::row()
            .with_spacing(4.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        if post.is_pinned {
            labels.add_child(icon(Icon::Pin, style.muted, small - 1.));
            labels.add_child(
                Text::new_inline("Pinned", style.font, small - 1.)
                    .with_color(style.muted)
                    .finish(),
            );
        }
        if data.actions.saved {
            let label = if post.is_pinned { "· Saved" } else { "Saved" };
            labels.add_child(
                Text::new_inline(label, style.font, small - 1.)
                    .with_color(style.muted)
                    .finish(),
            );
        }
        content.add_child(labels.finish());
    }
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
            let post = post.id.clone();
            let label = "replied in a thread";
            header.add_child(
                Text::new_inline(label, style.font, small)
                    .with_color(style.palette.link)
                    .with_clickable_char_range(0..label.chars().count(), move |_, ctx, _| {
                        ctx.dispatch_typed_action(ChannelAction::Post(PostAction::Reply {
                            post: post.clone(),
                        }))
                    })
                    .finish(),
            );
        }
        content.add_child(header.finish());
    }
    if let Some((editing, editor)) = editing {
        content.add_child(edit_box(editing, editor, style));
    } else {
        let doc = post_doc(shared, post);
        let renderer = MessageRenderer {
            palette: &style.palette,
            custom_emoji: &shared.custom_emoji,
            on_click: click_handler(),
            muted: post.is_system() || data.pending == Some(Pending::Sending),
        };
        content.add_child(renderer.render(&doc));
    }
    if let (Some((tooltip, history)), None) = (&data.edited, editing) {
        content.add_child(edited_indicator(
            shared, style, &post.id, tooltip, *history, appearance,
        ));
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
    let add_chip = data.actions.emoji_picker && editing.is_none();
    if !data.reactions.is_empty() {
        let mut chips = Wrap::row().with_spacing(4.).with_run_spacing(4.);
        let side = (small * 1.3).round();
        for chip in &data.reactions {
            chips.add_child(reaction_chip(shared, style, data, chip, side, appearance));
        }
        if add_chip {
            chips.add_child(add_reaction_chip(shared, style, &post.id, side));
        }
        content.add_child(chips.finish());
    }
    if data.replies > 0 {
        let label = timeline::replies_label(data.replies as usize);
        let length = label.chars().count();
        let post = post.id.clone();
        content.add_child(
            Flex::row()
                .with_spacing(6.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(icon(Icon::MessageChatSquare, style.palette.link, small))
                .with_child(
                    Text::new_inline(label, style.font, small)
                        .with_color(style.palette.link)
                        .with_clickable_char_range(0..length, move |_, ctx, _| {
                            ctx.dispatch_typed_action(ChannelAction::Post(PostAction::Reply {
                                post: post.clone(),
                            }))
                        })
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

/// The editor in place of a message being edited, with the keys to save and cancel under it
/// (or why it can't be saved).
fn edit_box(
    editing: &Editing,
    editor: &ViewHandle<EditorView>,
    style: &RowStyle,
) -> Box<dyn Element> {
    let border = if editing.error.is_some() {
        style.error
    } else {
        style.palette.link
    };
    let field = Container::new(
        ConstrainedBox::new(ChildView::new(editor).finish())
            .with_max_height(MAX_EDIT_HEIGHT)
            .finish(),
    )
    .with_horizontal_padding(10.)
    .with_vertical_padding(8.)
    .with_border(Border::all(1.).with_border_color(border))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
    let (note, color) = match &editing.error {
        Some(error) => (error.clone(), style.error),
        None => (editing.hint.to_owned(), style.muted),
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.)
        .with_child(field.finish())
        .with_child(
            Text::new(note, style.font, style.size - 2.)
                .with_color(color)
                .finish(),
        )
        .finish()
}

/// A reaction chip: the emoji and its count, marked when one is mine; a click adds or
/// removes mine, hovering shows who reacted.
fn reaction_chip(
    shared: &Shared,
    style: &RowStyle,
    data: &PostData,
    chip: &ReactionChip,
    side: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let small = style.size - 2.;
    let name = &chip.name;
    let emoji = match mm_emoji::by_name(name) {
        Some(emoji) => EmojiRef::System {
            name: name.clone(),
            image: emoji.image(),
        },
        None => EmojiRef::Custom { name: name.clone() },
    };
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
    let clickable = if chip.mine {
        data.actions.unreact
    } else {
        data.actions.react
    };
    let state = shared
        .chips
        .borrow_mut()
        .entry((data.post.id.clone(), name.clone()))
        .or_default()
        .clone();
    let (mine, count) = (chip.mine, chip.count);
    let tooltip = appearance
        .ui_builder()
        .tool_tip(chip.tooltip.clone())
        .build()
        .finish();
    let (link, text) = (style.chat.link, style.text);
    let font = style.font;
    let mut hoverable = Hoverable::new(state, move |state| {
        let (background, border) = match (mine, clickable && state.is_hovered()) {
            (true, false) => (fade(link, 0.12), link),
            (true, true) => (fade(link, 0.2), link),
            (false, false) => (fade(text, 0.06), fade(text, 0.06)),
            (false, true) => (fade(text, 0.1), fade(text, 0.3)),
        };
        let chip = Container::new(
            Flex::row()
                .with_spacing(4.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(image)
                .with_child(
                    Text::new_inline(count.to_string(), font, small)
                        .with_color(if mine { link } else { text })
                        .finish(),
                )
                .finish(),
        )
        .with_horizontal_padding(6.)
        .with_vertical_padding(2.)
        .with_background_color(background)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish();
        if !state.is_hovered() {
            return chip;
        }
        let mut stack = Stack::new().with_child(chip);
        stack.add_positioned_overlay_child(
            tooltip,
            warpui::elements::OffsetPositioning::offset_from_parent(
                pathfinder_geometry::vector::vec2f(0., -4.),
                warpui::elements::ParentOffsetBounds::WindowByPosition,
                warpui::elements::ParentAnchor::TopLeft,
                warpui::elements::ChildAnchor::BottomLeft,
            ),
        );
        stack.finish()
    });
    if clickable {
        let (post, emoji) = (data.post.id.clone(), name.clone());
        hoverable = hoverable
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ChannelAction::Post(PostAction::ToggleReaction {
                    post: post.clone(),
                    emoji: emoji.clone(),
                }))
            });
    }
    hoverable.finish()
}

/// "(edited)" under a message: when, on hover; on my post a click shows the edit history.
fn edited_indicator(
    shared: &Shared,
    style: &RowStyle,
    post: &PostId,
    tooltip: &str,
    history: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let state = shared
        .chips
        .borrow_mut()
        .entry((post.clone(), "(edited)".to_owned()))
        .or_default()
        .clone();
    let tooltip = appearance
        .ui_builder()
        .tool_tip(tooltip.to_owned())
        .build()
        .finish();
    let (font, size, muted, link) = (style.font, style.size - 2., style.muted, style.palette.link);
    let mut indicator = Hoverable::new(state, move |state| {
        let color = if history && state.is_hovered() {
            link
        } else {
            muted
        };
        let text = Text::new_inline("(edited)", font, size)
            .with_color(color)
            .finish();
        if !state.is_hovered() {
            return text;
        }
        let mut stack = Stack::new().with_child(text);
        stack.add_positioned_overlay_child(
            tooltip,
            warpui::elements::OffsetPositioning::offset_from_parent(
                pathfinder_geometry::vector::vec2f(0., -4.),
                warpui::elements::ParentOffsetBounds::WindowByPosition,
                warpui::elements::ParentAnchor::TopLeft,
                warpui::elements::ChildAnchor::BottomLeft,
            ),
        );
        stack.finish()
    });
    if history {
        let post = post.clone();
        indicator = indicator
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ChannelAction::Post(PostAction::EditHistory {
                    post: post.clone(),
                }))
            });
    }
    // In a row, so only the word takes the pointer.
    Flex::row().with_child(indicator.finish()).finish()
}

/// The `+` chip after a post's reactions: opens the emoji picker.
fn add_reaction_chip(
    shared: &Shared,
    style: &RowStyle,
    post: &PostId,
    side: f32,
) -> Box<dyn Element> {
    let state = shared
        .chips
        .borrow_mut()
        .entry((post.clone(), "+".to_owned()))
        .or_default()
        .clone();
    let (text, muted) = (style.text, style.muted);
    let chip = Hoverable::new(state, move |state| {
        let (background, border, color) = if state.is_hovered() {
            (fade(text, 0.1), fade(text, 0.3), text)
        } else {
            (fade(text, 0.06), fade(text, 0.06), muted)
        };
        Container::new(icon(Icon::FaceSmile, color, side))
            .with_horizontal_padding(6.)
            .with_vertical_padding(2.)
            .with_background_color(background)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click({
        let post = post.clone();
        let posts = shared.posts;
        move |ctx, _, _| {
            let action = posts.open_picker(ctx, post.clone(), PickerAnchor::Chip);
            ctx.dispatch_typed_action(action)
        }
    })
    .finish();
    warpui::elements::SavePosition::new(chip, &shared.posts.add_reaction_position(post)).finish()
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
            // Back from a dialog (Delete, after saving empty text): to the open edit.
            if self.editing.is_some() {
                ctx.focus(&self.edit_editor);
            } else {
                ctx.focus(&self.composer);
            }
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let Some(chat) = self.chat() else {
            return Empty::new().finish();
        };
        let style = Rc::new(row_style(appearance));
        let header = self.render_header(&style.chat, appearance);
        let shared = self.shared.clone();
        let row_style = style.clone();
        let row_state = RowState {
            flash: self.flash.clone(),
            menu: self.menu.as_ref().map(|menu| menu.post.clone()),
            picker: self.picking.as_ref().map(|(post, _, _)| post.clone()),
            thread: self.root().is_some(),
            editing: self.editing.clone(),
            editor: self.edit_editor.clone(),
        };
        let list = TimelineList::new(chat.list.clone(), move |index, app| {
            render_row(
                &shared,
                &row_style,
                index,
                &row_state,
                Appearance::as_ref(app),
            )
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
            // Positioned, not in a full-size `Container`: a container covers its whole area for
            // hit testing, and the timeline under it then ignored the wheel and hid its
            // scrollbar.
            stack.add_positioned_child(
                button,
                warpui::elements::OffsetPositioning::offset_from_parent(
                    pathfinder_geometry::vector::vec2f(0., -12.),
                    warpui::elements::ParentOffsetBounds::ParentByPosition,
                    warpui::elements::ParentAnchor::BottomMiddle,
                    warpui::elements::ChildAnchor::BottomMiddle,
                ),
            );
        }
        if let Some(menu) = &self.menu {
            stack.add_positioned_overlay_child(
                menu.render(self.posts, appearance),
                menu.positioning(self.posts),
            );
        }
        if let Some((post, anchor, up)) = &self.picking {
            let picker = warpui::elements::Dismiss::new(ChildView::new(&self.picker).finish())
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(ChannelAction::Post(PostAction::ClosePicker))
                })
                .finish();
            stack.add_positioned_overlay_child(
                picker,
                self.posts.picker_positioning(post, *anchor, *up),
            );
        }
        let timeline =
            warpui::elements::SavePosition::new(stack.finish(), &self.posts.root_position())
                .finish();
        let column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(Shrinkable::new(1., timeline).finish())
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
        let Some(key) = self.key.clone() else {
            return;
        };
        let channel = match &key {
            ChatKey::Channel(channel) => Some(channel.clone()),
            ChatKey::Thread(_) => None,
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
                let Some(channel) = channel else { return };
                let command = match kind {
                    HoleKind::Older => Command::LoadOlder(channel),
                    HoleKind::Newer => Command::LoadNewer(channel),
                    HoleKind::Gap | HoleKind::Replies => return,
                };
                Session::as_ref(ctx).send(command);
            }
            ChannelAction::JumpToNewest => {
                if let Some(chat) = self.chat() {
                    chat.list.scroll_to_bottom();
                }
                // Opened at the first unread post: the newest page may not be loaded yet.
                if let Some(channel) = channel {
                    Session::as_ref(ctx).send(Command::LoadNewer(channel));
                }
                ctx.notify();
            }
            // `Esc` closes the post menu first.
            ChannelAction::MarkRead if self.menu.is_some() || self.picking.is_some() => {
                self.menu = None;
                self.close_picker(false, ctx);
                ctx.notify();
            }
            // The right panel closes; a channel is marked read.
            ChannelAction::MarkRead if self.mode == ChatMode::Panel => {
                ctx.emit(ChannelEvent::Close)
            }
            ChannelAction::MarkRead => {
                if let Some(channel) = channel {
                    Session::as_ref(ctx).send(Command::MarkRead(channel));
                }
            }
            ChannelAction::Close => ctx.emit(ChannelEvent::Close),
            ChannelAction::OpenInTab => {
                if let Some(root) = self.root().cloned() {
                    ctx.emit(ChannelEvent::OpenInTab(root));
                }
            }
            ChannelAction::ToggleFollow => {
                if let (Some(root), Some(following)) = (self.root().cloned(), self.following) {
                    Session::as_ref(ctx).send(Command::FollowThread {
                        root,
                        follow: !following,
                    });
                }
            }
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
                if let Some(chat) = self.chat() {
                    chat.list.scroll_page(*action == ChannelAction::PageUp);
                }
                ctx.notify();
            }
            ChannelAction::Post(action) => self.post_action(action, ctx),
        }
    }
}

impl ChannelView {
    fn post_action(&mut self, action: &PostAction, ctx: &mut ViewContext<Self>) {
        let session = Session::as_ref(ctx);
        let Some(store) = session.store().cloned() else {
            return;
        };
        let server = session
            .active()
            .map(|active| active.server.as_str().to_owned())
            .unwrap_or_default();
        match action {
            PostAction::OpenMenu { post, at, up } => {
                let Ok(store) = store.read() else { return };
                let Some(known) = store.posts.get(post) else {
                    return;
                };
                let mut actions = store.post_actions(post, now_ms());
                // In a thread the composer replies.
                actions.reply &= self.root().is_none();
                self.menu = actions.any().then(|| {
                    PostMenu::new(known, *at, *up, actions, thread_replies(&store, known))
                });
                ctx.notify();
            }
            PostAction::CloseMenu => {
                self.menu = None;
                ctx.notify();
            }
            PostAction::OpenPicker { post, anchor, up } => {
                self.menu = None;
                self.picking = Some((post.clone(), *anchor, *up));
                self.picker.update(ctx, |picker, ctx| picker.open(ctx));
                ctx.focus(&self.picker);
                ctx.notify();
            }
            PostAction::ClosePicker => self.close_picker(false, ctx),
            PostAction::Reply { post } => self.reply(post, ctx),
            PostAction::EditHistory { post } => {
                ctx.emit(ChannelEvent::ShowEditHistory(post.clone()))
            }
            PostAction::ToggleReaction { post, emoji } => {
                let Ok(store) = store.read() else { return };
                let actions = store.post_actions(post, now_ms());
                let mine = store
                    .posts
                    .get(post)
                    .and_then(|post| post.metadata.as_ref())
                    .is_some_and(|metadata| {
                        metadata
                            .reactions
                            .iter()
                            .any(|r| r.user_id == store.me && r.emoji_name == *emoji)
                    });
                drop(store);
                if mine && actions.unreact {
                    Session::as_ref(ctx).send(Command::React {
                        post: post.clone(),
                        emoji: emoji.clone(),
                        add: false,
                    });
                } else if !mine && actions.react {
                    self.add_reaction(post, emoji, ctx);
                }
            }
            PostAction::Choose(item) => {
                let Some(menu) = self.menu.take() else { return };
                ctx.notify();
                let post = menu.post.clone();
                let actions = menu.actions;
                let command = match item {
                    PostMenuItem::Follow => Some(Command::FollowThread {
                        root: post,
                        follow: !actions.following,
                    }),
                    PostMenuItem::MarkUnread => Some(Command::MarkUnread {
                        post,
                        in_thread: false,
                    }),
                    PostMenuItem::Save => Some(Command::SavePost {
                        post,
                        saved: !actions.saved,
                    }),
                    PostMenuItem::Pin => Some(Command::PinPost {
                        post,
                        pinned: !actions.pinned,
                    }),
                    PostMenuItem::CopyLink | PostMenuItem::CopyText => {
                        let Ok(store) = store.read() else { return };
                        let text = if *item == PostMenuItem::CopyText {
                            store.posts.get(&post).map(|post| post.message.clone())
                        } else {
                            let team = store
                                .posts
                                .get(&post)
                                .and_then(|known| channel_team(&store, &known.channel_id));
                            crate::channel_info::post_link(&store, &post, team.as_ref(), &server)
                        };
                        drop(store);
                        if let Some(text) = text {
                            ctx.clipboard()
                                .write(warpui::clipboard::ClipboardContent::plain_text(text));
                        }
                        None
                    }
                    PostMenuItem::Edit => {
                        self.start_edit(&post, ctx);
                        None
                    }
                    PostMenuItem::Reply => {
                        self.reply(&post, ctx);
                        None
                    }
                    PostMenuItem::Delete => {
                        ctx.emit(ChannelEvent::ConfirmDelete {
                            post,
                            replies: menu.replies,
                        });
                        None
                    }
                };
                if let Some(command) = command {
                    Session::as_ref(ctx).send(command);
                }
            }
        }
    }
}

impl ChannelView {
    /// Reply on a post: its thread opens in the right panel; in a thread, the composer takes
    /// the keyboard.
    fn reply(&mut self, post: &PostId, ctx: &mut ViewContext<Self>) {
        if self.root().is_some() {
            self.menu = None;
            ctx.focus(&self.composer);
            ctx.notify();
            return;
        }
        let root = Session::as_ref(ctx).store().and_then(|store| {
            let store = store.read().ok()?;
            let known = store.posts.get(post)?;
            Some(if known.is_root() {
                known.id.clone()
            } else {
                known.root_id.clone()
            })
        });
        if let Some(root) = root {
            // The click that got here (a reply count) kept the keyboard for the timeline; the
            // thread takes it instead, and coming back lands in the composer.
            self.focus_timeline = false;
            ctx.emit(ChannelEvent::OpenThread(root));
        }
    }

    /// Adds my reaction (`addReaction`): nothing if it is there already, the limit dialog if
    /// the post has as many distinct emoji as the server allows.
    fn add_reaction(&mut self, post: &PostId, emoji: &str, ctx: &mut ViewContext<Self>) {
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else { return };
        if !store.post_actions(post, now_ms()).react {
            return;
        }
        let mine = store
            .posts
            .get(post)
            .and_then(|post| post.metadata.as_ref())
            .is_some_and(|metadata| {
                metadata
                    .reactions
                    .iter()
                    .any(|r| r.user_id == store.me && r.emoji_name == emoji)
            });
        let allowed = store.reaction_allowed(post, emoji);
        drop(store);
        if mine {
            return;
        }
        if !allowed {
            ctx.emit(ChannelEvent::ReactionLimit);
            return;
        }
        Session::as_ref(ctx).send(Command::React {
            post: post.clone(),
            emoji: emoji.to_owned(),
            add: true,
        });
    }

    /// Closes the emoji picker; `refocus`: the keyboard goes back to the edit or the composer
    /// (after a pick or Esc; a click elsewhere puts it where the click was).
    fn close_picker(&mut self, refocus: bool, ctx: &mut ViewContext<Self>) {
        if self.picking.take().is_none() {
            return;
        }
        if refocus {
            if self.editing.is_some() {
                ctx.focus(&self.edit_editor);
            } else {
                ctx.focus(&self.composer);
            }
        }
        ctx.notify();
    }

    /// `↑` in the empty composer: edits my newest post in the chat, if I still may.
    fn edit_last(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(me) = Session::as_ref(ctx)
            .store()
            .and_then(|store| Some(store.read().ok()?.me.clone()))
        else {
            return;
        };
        let posts = self.shared.rows.iter().filter_map(|row| match row {
            RowData::Post(data) => Some((&*data.post, data.pending.is_some())),
            _ => None,
        });
        if let Some(post) = timeline::latest_to_edit(posts, &me) {
            self.start_edit(&post, ctx);
        }
    }

    /// Opens the editor in place of a post's message, with the message (in Markdown) and
    /// the cursor at its end, and scrolls to the post if it is off screen.
    fn start_edit(&mut self, post: &PostId, ctx: &mut ViewContext<Self>) {
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else { return };
        if !store.post_actions(post, now_ms()).edit {
            return;
        }
        let Some(original) = store.posts.get(post).map(|known| known.message.clone()) else {
            return;
        };
        let hint = match (
            EnterPrefs::of(&store).send_on_ctrl_enter,
            cfg!(target_os = "macos"),
        ) {
            (false, _) => "Enter to save · Esc to cancel",
            (true, false) => "Ctrl+Enter to save · Esc to cancel",
            (true, true) => "⌘+Enter to save · Esc to cancel",
        };
        drop(store);
        self.menu = None;
        self.edit_editor.update(ctx, |editor, ctx| {
            editor.system_reset_buffer_text(&original, ctx);
            editor.move_to_buffer_end(ctx);
        });
        self.editing = Some(Editing {
            post: post.clone(),
            original,
            hint,
            error: None,
        });
        if let Some(chat) = self.chat() {
            let index = self
                .shared
                .rows
                .iter()
                .position(|row| matches!(row, RowData::Post(data) if data.post.id == *post));
            let visible = chat.viewport.first..=chat.viewport.last;
            if index.is_some_and(|index| !visible.contains(&index)) {
                chat.list
                    .scroll_to(timeline::RowKey::Post(post.clone()), SHOW_POST_OFFSET);
            }
        }
        ctx.focus(&self.edit_editor);
        ctx.notify();
    }

    fn on_edit_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Enter => self.save_edit(EnterKey::Plain, ctx),
            EditorEvent::CtrlEnter | EditorEvent::CmdEnter => {
                self.save_edit(EnterKey::CtrlOrCmd, ctx)
            }
            EditorEvent::Escape => self.stop_edit(ctx),
            EditorEvent::Edited(origin) if origin.is_user() => {
                let Some(editing) = &mut self.editing else {
                    return;
                };
                let text = self.edit_editor.as_ref(ctx).buffer_text(ctx);
                let Some(max) = Session::as_ref(ctx)
                    .store()
                    .and_then(|store| Some(composer::max_post_size(&*store.read().ok()?)))
                else {
                    return;
                };
                let error = composer::length_error(&text, max);
                if error != editing.error {
                    editing.error = error;
                    ctx.notify();
                }
            }
            _ => {}
        }
    }

    /// Enter in the editor: a new line (inside a code block, or per the Enter preference),
    /// or save. Empty text asks to delete the post instead; unchanged text only closes the
    /// editor (as the webapp's `EditPost`).
    fn save_edit(&mut self, key: EnterKey, ctx: &mut ViewContext<Self>) {
        let Some(editing) = self.editing.clone() else {
            return;
        };
        let editor = self.edit_editor.as_ref(ctx);
        let text = editor.buffer_text(ctx);
        let caret = editor.start_byte_index_of_first_selection(ctx).as_usize();
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else { return };
        let prefs = EnterPrefs::of(&store);
        let max = composer::max_post_size(&store);
        let replies = store
            .posts
            .get(&editing.post)
            .map_or(0, |post| thread_replies(&store, post));
        drop(store);
        let message = match composer::on_enter(&text, caret, key, prefs) {
            OnEnter::NewLine => {
                self.edit_editor
                    .update(ctx, |editor, ctx| editor.user_insert("\n", ctx));
                return;
            }
            OnEnter::Send(message) => message.trim_end().to_owned(),
        };
        if message.trim().is_empty() {
            ctx.emit(ChannelEvent::ConfirmDelete {
                post: editing.post,
                replies,
            });
            return;
        }
        if message == editing.original.trim_end() {
            self.stop_edit(ctx);
            return;
        }
        if let Some(error) = composer::length_error(&message, max) {
            if let Some(editing) = &mut self.editing {
                editing.error = Some(error);
            }
            ctx.notify();
            return;
        }
        Session::as_ref(ctx).send(Command::EditPost {
            post: editing.post,
            message,
        });
        self.stop_edit(ctx);
    }

    /// Closes the editor; the keyboard goes back to the composer if the editor had it.
    fn stop_edit(&mut self, ctx: &mut ViewContext<Self>) {
        if self.editing.take().is_none() {
            return;
        }
        let focused = self.edit_editor.is_focused(ctx);
        self.edit_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx)
        });
        if focused {
            ctx.focus(&self.composer);
        }
        ctx.notify();
    }
}

/// Replies in a post's thread (of the post itself if it is the root).
fn thread_replies(store: &Store, post: &Post) -> i64 {
    if post.is_root() {
        post.reply_count
    } else {
        store
            .posts
            .get(&post.root_id)
            .map_or(post.reply_count, |root| root.reply_count)
    }
}
