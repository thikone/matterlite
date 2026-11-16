// SPDX-License-Identifier: AGPL-3.0-only

//! The window's root: the login screen while signed out, the workspace while signed in, and
//! the session's dialogs on top (account and logout, server reset, notices).

use std::time::{Duration, Instant};

use ml_workspace::{AccountBadge, Banner, ConnectionStatus, Workspace, WorkspaceEvent};
use mm_sync::{ConnectionState, ServerChange};
use ui_components::{Component as _, Options as _, button, dialog};
use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Flex,
    Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement, Radius, Rect,
    Stack, Text,
};
use warpui::keymap::{Context, FixedBinding};
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use ml_workspace::{ToolPanelTab, ViewPane};
use mm_sync::Command;

use ml_workspace::{TabBadge, TabLeading, TabSpec};
use mm_model::ChannelId;
use mm_store::{Store, StoreDelta};
use settings::Setting as _;
use warp_core::ui::icons::Icon;

use crate::account_theme::AccountThemeFollower;
use crate::channel_pane::{ChannelPane, ChannelPaneEvent};
use crate::channel_view::{ChannelEvent, ChannelView};
use crate::chats::{ChannelIcon, Presence};
use crate::chats_view::{ChatsEvent, ChatsView};
use crate::format::format_size;
use crate::history_pane::{HistoryEvent, HistoryPane};
use crate::login_view::LoginView;
use crate::session::{Active, Session, SessionEvent, SessionState};
use crate::settings::ChatSettings;
use crate::switcher_view::{SwitcherEvent, SwitcherView};
use crate::tabs::{ChatTabs, DEFAULT_MAX_TABS, Tab};

const DIALOG_WIDTH: f32 = 600.;
/// Errors shown at once, how long, and how wide.
const MAX_ERRORS: usize = 3;
const ERROR_TIME: Duration = Duration::from_secs(12);
const ERROR_WIDTH: f32 = 360.;
/// The switcher's distance from the top of the window.
const SWITCHER_TOP: f32 = 80.;
/// UI state key of the open tabs.
const TABS_KEY: &str = "tabs";
/// Keymap context while a dialog is open (Esc closes it).
const DIALOG_OPEN: &str = "Root_DialogOpen";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootAction {
    /// Alt+↑/↓ (`delta` ±1), Alt+Shift+↑/↓ with `unread_only`.
    StepChat {
        delta: isize,
        unread_only: bool,
    },
    /// Ctrl/Cmd+K.
    OpenSwitcher,
    CloseDialog,
    LogOut {
        delete_data: bool,
    },
    ResolveServerChange(ServerChange),
    DismissNotice,
    DismissError,
    PauseSync,
    ResumeSync,
    /// The window buttons on the login screen.
    Window(ml_workspace::WindowButton),
}

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;
    let step = |keys: &str, delta: isize, unread_only: bool| {
        FixedBinding::new(
            keys,
            RootAction::StepChat { delta, unread_only },
            id!("RootView") & !id!(DIALOG_OPEN),
        )
    };
    app.register_fixed_bindings([
        FixedBinding::new(
            "escape",
            RootAction::CloseDialog,
            id!("RootView") & id!(DIALOG_OPEN),
        ),
        FixedBinding::new(
            "cmdorctrl-k",
            RootAction::OpenSwitcher,
            id!("RootView") & !id!(DIALOG_OPEN),
        ),
        step("alt-up", -1, false),
        step("alt-down", 1, false),
        step("alt-shift-up", -1, true),
        step("alt-shift-down", 1, true),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialog {
    Account,
}

#[derive(Default)]
struct Buttons {
    sync: button::Button,
    log_out: button::Button,
    log_out_delete: button::Button,
    cancel: button::Button,
    start_fresh: button::Button,
    keep_cache: button::Button,
    ok: button::Button,
    dialog: dialog::Dialog,
}

pub struct RootView {
    login: ViewHandle<LoginView>,
    workspace: Option<ViewHandle<Workspace>>,
    chats: Option<ViewHandle<ChatsView>>,
    channel: Option<ViewHandle<ChannelView>>,
    channel_pane: Option<ViewHandle<ChannelPane>>,
    history: Option<ViewHandle<HistoryPane>>,
    switcher: Option<ViewHandle<SwitcherView>>,
    switcher_open: bool,
    tabs: ChatTabs,
    /// The saved tabs are restored once the store holds the account's chats.
    tabs_restored: bool,
    /// The chat the engine was last told to open.
    open_channel: Option<ChannelId>,
    dialog: Option<Dialog>,
    buttons: Buttons,
    /// The connection as last seen, when the next reconnect is due, and whether the
    /// connection was up this session (so "Reconnecting…" isn't shown at start).
    connection: Option<ConnectionState>,
    retry_at: Option<Instant>,
    was_online: bool,
    /// A one-second tick runs while a countdown is shown.
    ticking: bool,
    /// What the banner's link does.
    banner_action: Option<Command>,
    /// Errors to show (newest last), each dismissed by hand or after a while.
    errors: Vec<String>,
    errors_shown: u64,
    error_close: MouseStateHandle,
    window: warpui::WindowId,
    window_buttons: ml_workspace::WindowButtonStates,
}

impl RootView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::StateChanged => me.on_state_changed(ctx),
            SessionEvent::StatusChanged => {
                me.restore_tabs(ctx);
                me.on_status_changed(ctx);
            }
            SessionEvent::Reloaded => {
                me.restore_tabs(ctx);
                me.drop_gone_tabs(ctx);
                me.sync_tabs(ctx);
            }
            SessionEvent::TypingChanged => me.sync_tabs(ctx),
            SessionEvent::Error(message) => me.show_error(message.clone(), ctx),
            SessionEvent::Changed(deltas) => {
                if deltas
                    .iter()
                    .any(|delta| matches!(delta, StoreDelta::ChannelRemoved(_)))
                {
                    me.drop_gone_tabs(ctx);
                }
                if deltas.iter().any(affects_tabs) {
                    me.sync_tabs(ctx);
                }
            }
            _ => {}
        });
        let login = ctx.add_typed_action_view(LoginView::new);
        let mut root = Self {
            login,
            workspace: None,
            chats: None,
            channel: None,
            channel_pane: None,
            history: None,
            switcher: None,
            switcher_open: false,
            tabs: ChatTabs::new(DEFAULT_MAX_TABS),
            tabs_restored: false,
            open_channel: None,
            dialog: None,
            buttons: Buttons::default(),
            connection: None,
            retry_at: None,
            was_online: false,
            ticking: false,
            banner_action: None,
            errors: Vec::new(),
            errors_shown: 0,
            error_close: MouseStateHandle::default(),
            window: ctx.window_id(),
            window_buttons: Default::default(),
        };
        ctx.subscribe_to_model(&ml_workspace::WindowFrame::handle(ctx), |_, _, _, ctx| {
            ctx.notify()
        });
        ml_workspace::window_controls::show_native_buttons(ctx, root.window);
        root.on_state_changed(ctx);
        root
    }

    fn on_state_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.dialog = None;
        self.connection = None;
        self.retry_at = None;
        self.was_online = false;
        self.errors.clear();
        self.switcher_open = false;
        self.tabs = ChatTabs::new(max_tabs(ctx));
        self.tabs_restored = false;
        self.open_channel = None;
        if Session::as_ref(ctx).active().is_some() {
            // A fresh workspace per sign-in: nothing of another account stays on screen.
            let workspace = ctx.add_typed_action_view(Workspace::new);
            ctx.subscribe_to_view(&workspace, |me, _, event, ctx| {
                me.on_workspace_event(event, ctx)
            });
            let switcher = ctx.add_typed_action_view(SwitcherView::new);
            ctx.subscribe_to_view(&switcher, |me, _, event, ctx| match event {
                SwitcherEvent::Chosen(channel) => {
                    me.close_switcher(ctx);
                    me.open_chat(channel.clone(), ctx);
                }
                SwitcherEvent::Dismissed => me.close_switcher(ctx),
            });
            self.switcher = Some(switcher);
            let chats = ctx.add_typed_action_view(ChatsView::new);
            ctx.subscribe_to_view(&chats, |me, _, event, ctx| me.on_chats_event(event, ctx));
            let pane = ViewPane(chats.clone());
            workspace.update(ctx, |workspace, ctx| {
                workspace.set_tool_pane(ToolPanelTab::Chats, Some(Box::new(pane)), ctx)
            });
            let channel = ctx.add_typed_action_view(ChannelView::new);
            ctx.subscribe_to_view(&channel, |me, _, event, ctx| match event {
                ChannelEvent::OpenChannel(channel) => me.open_chat(channel.clone(), ctx),
                ChannelEvent::ClearFilter => {
                    if let Some(history) = &me.history {
                        history.update(ctx, |history, ctx| history.clear_filter(ctx));
                    }
                }
            });
            let content = ViewPane(channel.clone());
            workspace.update(ctx, |workspace, ctx| {
                workspace.set_content(Some(Box::new(content)), ctx)
            });
            self.channel = Some(channel);
            let channel_pane = ctx.add_typed_action_view(ChannelPane::new);
            ctx.subscribe_to_view(&channel_pane, |me, _, event, ctx| match event {
                ChannelPaneEvent::ShowPost(post) => me.show_post(post.clone(), ctx),
            });
            let pane = ViewPane(channel_pane.clone());
            workspace.update(ctx, |workspace, ctx| {
                workspace.set_tool_pane(ToolPanelTab::Channel, Some(Box::new(pane)), ctx)
            });
            self.channel_pane = Some(channel_pane);
            let history = ctx.add_typed_action_view(HistoryPane::new);
            ctx.subscribe_to_view(&history, |me, _, event, ctx| match event {
                HistoryEvent::ShowPost(post) => me.show_post(post.clone(), ctx),
                HistoryEvent::Filter(filter) => {
                    if let Some(view) = &me.channel {
                        let filter = filter.clone();
                        view.update(ctx, |view, ctx| view.set_filter(filter, ctx));
                    }
                }
            });
            let pane = ViewPane(history.clone());
            workspace.update(ctx, |workspace, ctx| {
                workspace.set_tool_pane(ToolPanelTab::History, Some(Box::new(pane)), ctx)
            });
            self.history = Some(history);
            let team = chats.as_ref(ctx).team().cloned();
            if let Some(pane) = &self.channel_pane {
                let team = team.clone();
                pane.update(ctx, |pane, ctx| pane.set_team(team, ctx));
            }
            AccountThemeFollower::handle(ctx)
                .update(ctx, |follower, ctx| follower.set_team(team, ctx));
            ctx.focus(&workspace);
            self.workspace = Some(workspace);
            self.chats = Some(chats);
            self.restore_tabs(ctx);
            self.on_status_changed(ctx);
        } else {
            self.workspace = None;
            self.chats = None;
            self.channel = None;
            self.channel_pane = None;
            self.history = None;
            self.switcher = None;
            ctx.focus(&self.login);
        }
        ctx.notify();
    }

    fn on_chats_event(&mut self, event: &ChatsEvent, ctx: &mut ViewContext<Self>) {
        match event {
            ChatsEvent::Open(channel) => self.open_chat(channel.clone(), ctx),
            ChatsEvent::TeamChanged(team) => {
                if let Some(pane) = &self.channel_pane {
                    let team = team.clone();
                    pane.update(ctx, |pane, ctx| pane.set_team(team, ctx));
                }
                let team = team.clone();
                AccountThemeFollower::handle(ctx)
                    .update(ctx, |follower, ctx| follower.set_team(team, ctx));
            }
            ChatsEvent::Dismissed => {
                if let Some(workspace) = &self.workspace {
                    ctx.focus(workspace);
                }
            }
        }
    }

    fn on_workspace_event(&mut self, event: &WorkspaceEvent, ctx: &mut ViewContext<Self>) {
        match *event {
            WorkspaceEvent::AccountMenu => {
                self.dialog = Some(Dialog::Account);
                ctx.notify();
            }
            WorkspaceEvent::NewTab => self.open_switcher(ctx),
            WorkspaceEvent::BannerAction => {
                if let Some(command) = self.banner_action.clone() {
                    Session::as_ref(ctx).send(command);
                }
            }
            WorkspaceEvent::ActivateTab(index) => {
                self.tabs.activate(index);
                self.after_tabs_change(ctx);
                self.focus_chat(ctx);
            }
            WorkspaceEvent::CloseTab(index) => {
                self.tabs.close(index);
                self.after_tabs_change(ctx);
            }
            WorkspaceEvent::CloseOtherTabs(index) => {
                self.tabs.close_others(index);
                self.after_tabs_change(ctx);
            }
            WorkspaceEvent::TogglePinTab(index) => {
                self.tabs.toggle_pin(index);
                self.after_tabs_change(ctx);
            }
            WorkspaceEvent::ReopenClosedTab => {
                self.tabs.reopen();
                self.after_tabs_change(ctx);
            }
        }
    }

    /// Focuses the chat's tab, or opens one (MatterLite.md §10.7).
    fn open_chat(&mut self, channel: ChannelId, ctx: &mut ViewContext<Self>) {
        self.tabs.set_cap(max_tabs(ctx));
        self.tabs.open(channel);
        self.after_tabs_change(ctx);
        self.focus_chat(ctx);
    }

    /// Shows a post in the open chat (from the Channel tab's Pinned list or History).
    fn show_post(&mut self, post: mm_model::PostId, ctx: &mut ViewContext<Self>) {
        if let Some(view) = &self.channel {
            let shown = view.update(ctx, |view, ctx| view.show_post(&post, ctx));
            if !shown {
                log::info!("post {post} isn't loaded in the chat");
            }
        }
    }

    /// Moves the focus to the open chat (its composer, once there is one).
    fn focus_chat(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(workspace) = &self.workspace {
            workspace.update(ctx, |workspace, ctx| workspace.focus_content(ctx));
        }
    }

    /// After any change to the tabs: tell the engine which chat is open, show it in the Chats
    /// tab, save the tabs and redraw them.
    fn after_tabs_change(&mut self, ctx: &mut ViewContext<Self>) {
        let active = self.tabs.active_channel().cloned();
        if active != self.open_channel {
            Session::as_ref(ctx).send(match &active {
                Some(channel) => Command::OpenChannel(channel.clone()),
                None => Command::CloseChannel,
            });
            self.open_channel = active.clone();
        }
        if let Some(chats) = &self.chats {
            chats.update(ctx, |chats, ctx| chats.show_channel(active.as_ref(), ctx));
        }
        if let Some(view) = &self.channel {
            let open: Vec<ChannelId> = self
                .tabs
                .tabs()
                .iter()
                .map(|tab| tab.channel.clone())
                .collect();
            view.update(ctx, |view, ctx| {
                view.retain(&open);
                view.set_channel(active.clone(), ctx);
            });
        }
        if let Some(pane) = &self.channel_pane {
            let active = active.clone();
            pane.update(ctx, |pane, ctx| pane.set_channel(active, ctx));
        }
        if let Some(history) = &self.history {
            let active = active.clone();
            history.update(ctx, |history, ctx| history.set_channel(active, ctx));
        }
        if self.tabs_restored {
            let json = self.tabs.to_json();
            Session::handle(ctx)
                .update(ctx, |session, _| session.set_ui_state(TABS_KEY, Some(json)));
        }
        self.sync_tabs(ctx);
        self.on_status_changed(ctx);
    }

    /// Restores the saved tabs once the account's chats are loaded.
    fn restore_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        if self.tabs_restored || self.workspace.is_none() {
            return;
        }
        let session = Session::as_ref(ctx);
        let Some(active) = session.active().filter(|active| active.hydrated) else {
            return;
        };
        let saved = active.ui_state(TABS_KEY).map(str::to_owned);
        let Some(store) = session.store().and_then(|store| store.read().ok()) else {
            return;
        };
        if store.channels.is_empty() {
            return;
        }
        let tabs = saved.map(|json| {
            ChatTabs::from_json(&json, max_tabs(ctx), |channel| {
                store.channel_members.contains_key(channel)
            })
        });
        drop(store);
        self.tabs_restored = true;
        if let Some(tabs) = tabs {
            self.tabs = tabs;
        }
        self.after_tabs_change(ctx);
        // Back where the user left off, with the keyboard in the chat.
        if self.tabs.active_channel().is_some() {
            self.focus_chat(ctx);
        }
    }

    /// Closes the tabs of chats I'm no longer in.
    fn drop_gone_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        let removed = match Session::as_ref(ctx)
            .store()
            .and_then(|store| store.read().ok())
        {
            Some(store) => self
                .tabs
                .retain(|channel| store.channel_members.contains_key(channel)),
            None => return,
        };
        if !removed.is_empty() {
            self.after_tabs_change(ctx);
        }
    }

    /// Relabels the tabs from the store (names, icons, unread counts).
    fn sync_tabs(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let theme = Appearance::as_ref(ctx).theme();
        let palette = theme.chat();
        let offline = theme.sub_text_color(theme.background()).into_solid();
        let active = self.tabs.active();
        let session = Session::as_ref(ctx);
        let specs: Vec<TabSpec> = match session.store().and_then(|store| store.read().ok()) {
            Some(store) => self
                .tabs
                .tabs()
                .iter()
                .enumerate()
                .map(|(index, tab)| {
                    let mut spec = tab_spec(&store, tab, Some(index) == active, &palette, offline);
                    spec.typing = session.active().is_some_and(|session| {
                        !session.typing(&tab.channel, &Default::default()).is_empty()
                    });
                    spec
                })
                .collect(),
            None => Vec::new(),
        };
        workspace.update(ctx, |workspace, ctx| workspace.set_tabs(specs, active, ctx));
    }

    fn open_switcher(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(switcher) = self.switcher.clone() else {
            return;
        };
        self.switcher_open = true;
        switcher.update(ctx, |switcher, ctx| switcher.reset(ctx));
        ctx.focus(&switcher);
        ctx.notify();
    }

    fn close_switcher(&mut self, ctx: &mut ViewContext<Self>) {
        if self.switcher_open {
            self.switcher_open = false;
            if let Some(workspace) = &self.workspace {
                ctx.focus(workspace);
            }
            ctx.notify();
        }
    }

    fn on_status_changed(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            ctx.notify();
            return;
        };
        let session = Session::as_ref(ctx);
        let Some(active) = session.active() else {
            return;
        };
        let badge = account_badge(active);
        let lines = status_lines(active, session);
        let connection = active.connection;
        let paused = active.sync.paused;
        let server = active
            .server
            .as_str()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_owned();
        if self.connection != Some(connection) {
            self.connection = Some(connection);
            self.retry_at = match connection {
                ConnectionState::Offline { retry_in } => Some(Instant::now() + retry_in),
                _ => None,
            };
            if connection == ConnectionState::Online {
                self.was_online = true;
            }
        }
        let (banner, action) = match connection {
            ConnectionState::Offline { .. } => {
                let seconds = self.retry_at.map_or(0, |at| {
                    at.saturating_duration_since(Instant::now())
                        .as_secs_f32()
                        .ceil() as u64
                });
                let text = if seconds > 0 {
                    format!("No connection to {server}. Retrying in {seconds} s.")
                } else {
                    format!("No connection to {server}. Reconnecting…")
                };
                self.start_ticking(ctx);
                (
                    Some(Banner {
                        text,
                        action: Some("Retry now".to_owned()),
                        warning: true,
                    }),
                    Some(Command::ReconnectNow),
                )
            }
            ConnectionState::Connecting if self.was_online => (
                Some(Banner {
                    text: format!("Reconnecting to {server}…"),
                    action: None,
                    warning: true,
                }),
                None,
            ),
            _ if paused => (
                Some(Banner {
                    text: "Background sync is paused; new messages still arrive.".to_owned(),
                    action: Some("Resume sync".to_owned()),
                    warning: false,
                }),
                Some(Command::ResumeSync),
            ),
            _ => (None, None),
        };
        self.banner_action = action;
        workspace.update(ctx, |workspace, ctx| {
            workspace.set_account(Some(badge), ctx);
            workspace.set_placeholder(lines, ctx);
            workspace.set_banner(banner, ctx);
        });
        ctx.notify();
    }

    /// Updates the countdown in the banner every second while offline.
    fn start_ticking(&mut self, ctx: &mut ViewContext<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        let _ = ctx.spawn(
            async { warpui::r#async::Timer::after(Duration::from_secs(1)).await },
            |me, _, ctx| {
                me.ticking = false;
                if matches!(me.connection, Some(ConnectionState::Offline { .. })) {
                    me.on_status_changed(ctx);
                }
            },
        );
    }

    /// Shows an error from the engine (a message not sent, a setting not saved) until it is
    /// dismissed or for a while.
    fn show_error(&mut self, message: String, ctx: &mut ViewContext<Self>) {
        if self.workspace.is_none() {
            return;
        }
        self.errors.retain(|error| *error != message);
        self.errors.push(message);
        if self.errors.len() > MAX_ERRORS {
            self.errors.remove(0);
        }
        self.errors_shown += 1;
        let shown = self.errors_shown;
        let _ = ctx.spawn(
            async { warpui::r#async::Timer::after(ERROR_TIME).await },
            move |me, _, ctx| {
                if me.errors_shown == shown {
                    me.errors.clear();
                    ctx.notify();
                }
            },
        );
        ctx.notify();
    }

    fn render_errors(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let chat = theme.chat();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text = theme.main_text_color(theme.surface_1()).into_solid();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.);
        for error in &self.errors {
            column.add_child(
                Text::new(error.clone(), font, size)
                    .with_color(text)
                    .finish(),
            );
        }
        let close = Hoverable::new(self.error_close.clone(), move |state| {
            let mut properties = warpui::fonts::Properties::default();
            if state.is_hovered() {
                properties = properties.weight(warpui::fonts::Weight::Bold);
            }
            Text::new_inline("Dismiss", font, size - 1.)
                .with_color(chat.link)
                .with_style(properties)
                .finish()
        })
        .with_cursor(warpui::platform::Cursor::PointingHand)
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(RootAction::DismissError))
        .finish();
        let card = Container::new(
            Flex::row()
                .with_spacing(12.)
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(
                    ConstrainedBox::new(column.finish())
                        .with_max_width(ERROR_WIDTH)
                        .finish(),
                )
                .with_child(close)
                .finish(),
        )
        .with_uniform_padding(12.)
        .with_background(theme.surface_1())
        .with_border(Border::left(4.).with_border_color(chat.error_text))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish();
        Container::new(Align::new(card).bottom_right().finish())
            .with_uniform_padding(20.)
            .with_padding_bottom(90.)
            .finish()
    }

    fn render_dialog(&self, active: &Active, app: &AppContext) -> Option<Box<dyn Element>> {
        let appearance = Appearance::as_ref(app);
        // The engine's questions come first: nothing syncs until they're answered.
        if active.server_changed {
            return Some(self.render_server_changed(active, appearance));
        }
        if let Some(notice) = active.notices.first() {
            return Some(self.render_notice(notice, appearance));
        }
        match self.dialog? {
            Dialog::Account => Some(self.render_account(active, app, appearance)),
        }
    }

    fn render_account(
        &self,
        active: &Active,
        app: &AppContext,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let user = &active.user;
        let mut lines = vec![format!(
            "Signed in as @{} on {}",
            user.username,
            active.server.as_str()
        )];
        let name = mm_store::full_name(user);
        if !name.is_empty() {
            lines.push(name);
        }
        lines.push(connection_text(active));
        lines.push(sync_text(active));
        if let Some(account) = Session::as_ref(app)
            .accounts()
            .iter()
            .find(|account| account.dir.ends_with(&active.account_id))
        {
            lines.push(format!("Local data: {}", format_size(account.size)));
        }
        lines.push(
            "Logging out keeps the messages cached on this computer, so the next login opens \
             at once. Delete them to free the space or when this computer isn't yours."
                .to_owned(),
        );
        let busy = active.logging_out;
        let (sync_label, sync_action) = if active.sync.paused {
            ("Resume sync", RootAction::ResumeSync)
        } else {
            ("Pause sync", RootAction::PauseSync)
        };
        let buttons = vec![
            self.button(
                &self.buttons.sync,
                sync_label,
                false,
                busy,
                sync_action,
                appearance,
            ),
            self.button(
                &self.buttons.cancel,
                "Cancel",
                false,
                busy,
                RootAction::CloseDialog,
                appearance,
            ),
            self.button(
                &self.buttons.log_out_delete,
                "Log out and delete local data",
                false,
                busy,
                RootAction::LogOut { delete_data: true },
                appearance,
            ),
            self.button(
                &self.buttons.log_out,
                "Log out",
                true,
                busy,
                RootAction::LogOut { delete_data: false },
                appearance,
            ),
        ];
        self.dialog("Account", lines, buttons, true, appearance)
    }

    fn render_server_changed(&self, active: &Active, appearance: &Appearance) -> Box<dyn Element> {
        let lines = vec![
            format!(
                "{} looks like a different server than the one these messages were cached from: \
                 it was reset, reinstalled or restored from a backup.",
                active.server.as_str()
            ),
            "Start fresh to download everything again; the old local copy is kept as an \
             archive folder. Keep the local copy only if the server was restored from a backup \
             of the same installation."
                .to_owned(),
        ];
        let buttons = vec![
            self.button(
                &self.buttons.keep_cache,
                "Keep the local copy",
                false,
                false,
                RootAction::ResolveServerChange(ServerChange::KeepCache),
                appearance,
            ),
            self.button(
                &self.buttons.start_fresh,
                "Start fresh",
                true,
                false,
                RootAction::ResolveServerChange(ServerChange::ArchiveAndStartFresh),
                appearance,
            ),
        ];
        self.dialog("The server has changed", lines, buttons, false, appearance)
    }

    fn render_notice(&self, notice: &str, appearance: &Appearance) -> Box<dyn Element> {
        let buttons = vec![self.button(
            &self.buttons.ok,
            "OK",
            true,
            false,
            RootAction::DismissNotice,
            appearance,
        )];
        self.dialog(
            "MatterLite",
            vec![notice.to_owned()],
            buttons,
            false,
            appearance,
        )
    }

    fn button(
        &self,
        button: &button::Button,
        label: &'static str,
        primary: bool,
        disabled: bool,
        action: RootAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme: &dyn button::Theme = if primary {
            &button::themes::Primary
        } else {
            &button::themes::Secondary
        };
        button.render(
            appearance,
            button::Params {
                content: button::Content::Label(label.into()),
                theme,
                options: button::Options {
                    disabled,
                    on_click: Some(Box::new(move |ctx, _, _| ctx.dispatch_typed_action(action))),
                    ..button::Options::default(appearance)
                },
            },
        )
    }

    fn dialog(
        &self,
        title: &'static str,
        lines: Vec<String>,
        buttons: Vec<Box<dyn Element>>,
        dismissible: bool,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text_color = theme.main_text_color(theme.surface_1()).into_solid();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        for line in lines {
            body.add_child(Text::new(line, font, size).with_color(text_color).finish());
        }
        let body = Container::new(body.finish())
            .with_horizontal_padding(dialog::HORIZONTAL_PADDING)
            .with_padding_bottom(dialog::BASE_PADDING)
            .finish();
        let footer = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(8.)
            .with_children(buttons)
            .finish();
        self.buttons.dialog.render(
            appearance,
            dialog::Params {
                title: title.into(),
                content: Box::new(move |_: &Appearance| body),
                options: dialog::Options {
                    width: Some(DIALOG_WIDTH),
                    on_dismiss: dismissible.then(|| -> dialog::DismissHandler {
                        std::sync::Arc::new(|ctx, _| {
                            ctx.dispatch_typed_action(RootAction::CloseDialog)
                        })
                    }),
                    dismiss_keystroke: None,
                    footer: Some(Box::new(move |_: &Appearance| footer)),
                },
            },
        )
    }
}

/// The account button: initials and a connection dot.
fn account_badge(active: &Active) -> AccountBadge {
    let user = &active.user;
    let initials: String = [&user.first_name, &user.last_name]
        .iter()
        .filter_map(|name| name.chars().next())
        .collect();
    let initials = if initials.is_empty() {
        user.username.chars().take(1).collect()
    } else {
        initials
    };
    AccountBadge {
        initials: initials.to_uppercase(),
        tooltip: format!("@{} · {}", user.username, connection_text(active)),
        status: match active.connection {
            ConnectionState::Online => ConnectionStatus::Online,
            ConnectionState::Connecting => ConnectionStatus::Connecting,
            ConnectionState::Offline { .. } => ConnectionStatus::Offline,
        },
    }
}

/// The eager sync's progress for the Account dialog.
fn sync_text(active: &Active) -> String {
    let sync = active.sync;
    if sync.paused {
        return "Background sync: paused (new messages still arrive)".to_owned();
    }
    if sync.queued + sync.running > 0 || sync.backfilling > 0 {
        format!(
            "Background sync: {} requests queued, {} running, {} chats loading older history",
            sync.queued, sync.running, sync.backfilling
        )
    } else if active.ready {
        "Background sync: up to date".to_owned()
    } else {
        "Background sync: waiting for the connection".to_owned()
    }
}

fn connection_text(active: &Active) -> String {
    match active.connection {
        ConnectionState::Online if active.ready => "Connected".to_owned(),
        ConnectionState::Online | ConnectionState::Connecting => "Connecting…".to_owned(),
        ConnectionState::Offline { retry_in } => {
            format!("Offline, retrying in {} s", retry_in.as_secs().max(1))
        }
    }
}

/// What the placeholder chat area shows until the chat views exist (M3 steps 3–6).
fn status_lines(active: &Active, session: &Session) -> Vec<String> {
    let mut lines = vec![format!(
        "@{} on {} · {}",
        active.user.username,
        active.server.as_str(),
        connection_text(active)
    )];
    if let Some(store) = session.store()
        && let Ok(store) = store.read()
    {
        lines.push(format!(
            "{} · {} · {} cached",
            count(store.teams.len(), "team"),
            count(store.channels.len(), "channel"),
            count(store.posts.len(), "post")
        ));
    }
    let sync = active.sync;
    if sync.paused {
        lines.push("Sync paused".to_owned());
    } else if sync.queued + sync.running > 0 || sync.backfilling > 0 {
        lines.push(format!(
            "Syncing: {} queued, {} running, {} channels backfilling",
            sync.queued, sync.running, sync.backfilling
        ));
    } else if active.ready {
        lines.push("Everything is synced.".to_owned());
    }
    lines
}

fn max_tabs(app: &AppContext) -> usize {
    *ChatSettings::as_ref(app).max_open_tabs.value()
}

fn affects_tabs(delta: &StoreDelta) -> bool {
    matches!(
        delta,
        StoreDelta::ChannelChanged(_)
            | StoreDelta::MemberChanged(_)
            | StoreDelta::MarkedRead { .. }
            | StoreDelta::StatusChanged(_)
            | StoreDelta::UserChanged(_)
            | StoreDelta::NamesChanged
            | StoreDelta::PreferencesChanged
            | StoreDelta::CrtChanged
    )
}

/// What a chat's tab shows: icon, name, and a mention count or unread dot (not on the active
/// tab, which is being read).
fn tab_spec(
    store: &Store,
    tab: &Tab,
    active: bool,
    palette: &ml_themes::ChatColors,
    offline: warpui::color::ColorU,
) -> TabSpec {
    let leading = match crate::chats::channel_icon(store, &tab.channel) {
        ChannelIcon::Public => TabLeading::Icon(Icon::Hash),
        ChannelIcon::Private => TabLeading::Icon(Icon::Lock),
        ChannelIcon::Archived => TabLeading::Icon(Icon::Archive),
        ChannelIcon::Group(members) => TabLeading::Count(members),
        ChannelIcon::Direct(presence) => match presence {
            Presence::Online => TabLeading::Status {
                color: palette.online,
                filled: true,
            },
            Presence::Away => TabLeading::Status {
                color: palette.away,
                filled: true,
            },
            Presence::Dnd => TabLeading::Status {
                color: palette.dnd,
                filled: true,
            },
            Presence::Offline => TabLeading::Status {
                color: offline,
                filled: false,
            },
        },
    };
    let unread = store.channel_unread(&tab.channel).unwrap_or_default();
    let badge = if active {
        TabBadge::None
    } else if unread.mentions > 0 {
        TabBadge::Mentions(unread.mentions)
    } else if unread.is_unread() {
        TabBadge::Dot
    } else {
        TabBadge::None
    };
    TabSpec {
        title: store
            .channel_name(&tab.channel)
            .unwrap_or_else(|| tab.channel.to_string()),
        leading,
        badge,
        unread: !active && unread.is_unread(),
        pinned: tab.pinned,
        typing: false,
    }
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

impl Entity for RootView {
    type Event = ();
}

impl View for RootView {
    fn ui_name() -> &'static str {
        "RootView"
    }

    fn keymap_context(&self, app: &AppContext) -> Context {
        let mut context = Self::default_keymap_context();
        if self.dialog.is_some()
            && Session::as_ref(app)
                .active()
                .is_some_and(|active| !active.server_changed && active.notices.is_empty())
        {
            context.set.insert(DIALOG_OPEN);
        }
        context
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let session = Session::as_ref(app);
        let (SessionState::Active(active), Some(workspace)) = (session.state(), &self.workspace)
        else {
            let login = ChildView::new(&self.login).finish();
            let buttons = ml_workspace::window_controls::render(
                self.window,
                &self.window_buttons,
                |button, ctx| ctx.dispatch_typed_action(RootAction::Window(button)),
                app,
            );
            return match buttons {
                Some(buttons) => Stack::new()
                    .with_child(login)
                    .with_child(Align::new(buttons).top_right().finish())
                    .finish(),
                None => login,
            };
        };
        let mut stack = Stack::new().with_child(ChildView::new(workspace).finish());
        if self.switcher_open
            && let Some(switcher) = &self.switcher
        {
            stack.add_child(
                Container::new(
                    Align::new(ChildView::new(switcher).finish())
                        .top_center()
                        .finish(),
                )
                .with_padding_top(SWITCHER_TOP)
                .finish(),
            );
        }
        if !self.errors.is_empty() {
            stack.add_child(self.render_errors(Appearance::as_ref(app)));
        }
        if let Some(dialog) = self.render_dialog(active, app) {
            let theme = Appearance::as_ref(app).theme();
            let mut scrim = theme.background().into_solid();
            scrim.a = 140;
            stack.add_child(Rect::new().with_background_color(scrim).finish());
            stack.add_child(Align::new(dialog).finish());
        }
        stack.finish()
    }
}

impl TypedActionView for RootView {
    type Action = RootAction;

    fn handle_action(&mut self, action: &RootAction, ctx: &mut ViewContext<Self>) {
        match *action {
            RootAction::StepChat { delta, unread_only } => {
                if let Some(chats) = &self.chats {
                    chats.update(ctx, |chats, ctx| chats.step(delta, unread_only, ctx));
                }
            }
            RootAction::OpenSwitcher => self.open_switcher(ctx),
            RootAction::CloseDialog => {
                if Session::as_ref(ctx)
                    .active()
                    .is_some_and(|active| active.logging_out)
                {
                    return;
                }
                self.dialog = None;
                if let Some(workspace) = &self.workspace {
                    ctx.focus(workspace);
                }
                ctx.notify();
            }
            RootAction::LogOut { delete_data } => {
                Session::handle(ctx).update(ctx, |session, ctx| session.log_out(delete_data, ctx));
            }
            RootAction::ResolveServerChange(answer) => {
                Session::handle(ctx).update(ctx, |session, ctx| {
                    session.resolve_server_change(answer, ctx)
                });
            }
            RootAction::Window(button) => match button {
                ml_workspace::WindowButton::Minimize => ctx.minimize_window(),
                ml_workspace::WindowButton::Maximize => ctx.toggle_maximized_window(),
                ml_workspace::WindowButton::Close => ctx.close_window(),
            },
            RootAction::DismissError => {
                self.errors.clear();
                ctx.notify();
            }
            RootAction::PauseSync => Session::as_ref(ctx).send(Command::PauseSync),
            RootAction::ResumeSync => Session::as_ref(ctx).send(Command::ResumeSync),
            RootAction::DismissNotice => {
                Session::handle(ctx).update(ctx, |session, ctx| session.dismiss_notice(ctx));
            }
        }
    }
}
