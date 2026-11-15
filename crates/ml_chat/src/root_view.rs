// SPDX-License-Identifier: AGPL-3.0-only

//! The window's root: the login screen while signed out, the workspace while signed in, and
//! the session's dialogs on top (account and logout, server reset, notices).

use ml_workspace::{AccountBadge, ConnectionStatus, Workspace, WorkspaceEvent};
use mm_sync::{ConnectionState, ServerChange};
use ui_components::{Component as _, Options as _, button, dialog};
use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Align, ChildView, Container, CrossAxisAlignment, Flex, MainAxisAlignment, MainAxisSize,
    ParentElement, Rect, Stack, Text,
};
use warpui::keymap::{Context, FixedBinding};
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::format::format_size;
use crate::login_view::LoginView;
use crate::session::{Active, Session, SessionEvent, SessionState};

const DIALOG_WIDTH: f32 = 460.;
/// Keymap context while a dialog is open (Esc closes it).
const DIALOG_OPEN: &str = "Root_DialogOpen";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootAction {
    CloseDialog,
    LogOut { delete_data: bool },
    ResolveServerChange(ServerChange),
    DismissNotice,
}

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        RootAction::CloseDialog,
        id!("RootView") & id!(DIALOG_OPEN),
    )]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialog {
    Account,
}

#[derive(Default)]
struct Buttons {
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
    dialog: Option<Dialog>,
    buttons: Buttons,
}

impl RootView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::StateChanged => me.on_state_changed(ctx),
            SessionEvent::StatusChanged => me.on_status_changed(ctx),
            _ => {}
        });
        let login = ctx.add_typed_action_view(LoginView::new);
        let mut root = Self {
            login,
            workspace: None,
            dialog: None,
            buttons: Buttons::default(),
        };
        root.on_state_changed(ctx);
        root
    }

    fn on_state_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.dialog = None;
        if Session::as_ref(ctx).active().is_some() {
            // A fresh workspace per sign-in: nothing of another account stays on screen.
            let workspace = ctx.add_typed_action_view(Workspace::new);
            ctx.subscribe_to_view(&workspace, |me, _, event, ctx| match event {
                WorkspaceEvent::AccountMenu => {
                    me.dialog = Some(Dialog::Account);
                    ctx.notify();
                }
            });
            ctx.focus(&workspace);
            self.workspace = Some(workspace);
            self.on_status_changed(ctx);
        } else {
            self.workspace = None;
            ctx.focus(&self.login);
        }
        ctx.notify();
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
        workspace.update(ctx, |workspace, ctx| {
            workspace.set_account(Some(badge), ctx);
            workspace.set_placeholder(lines, ctx);
        });
        ctx.notify();
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
        let buttons = vec![
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
            return ChildView::new(&self.login).finish();
        };
        let mut stack = Stack::new().with_child(ChildView::new(workspace).finish());
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
            RootAction::DismissNotice => {
                Session::handle(ctx).update(ctx, |session, ctx| session.dismiss_notice(ctx));
            }
        }
    }
}
