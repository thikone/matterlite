// SPDX-License-Identifier: AGPL-3.0-only

//! The login screen (MatterLite.md §10.2 `LoginView`, §8.7): server URL → the server's sign-in
//! options → login id and password → MFA code when asked; below the server field, the accounts
//! that have local data on this computer.

use std::path::PathBuf;

use ml_editor::{
    EditorView, Event as EditorEvent, NavigationKey, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};
use mm_api::LoginRequest;
use mm_cache::StoredAccount;
use mm_sync::{AuthError, ServerInfo};
use ui_components::{Component as _, Options as _, button};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Expanded, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement,
    Radius, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::format::{format_last_used, format_size};
use crate::session::{Session, SessionEvent, SessionState};

const CARD_WIDTH: f32 = 380.;
const FIELD_FONT_SIZE: f32 = 14.;
const FIELD_PADDING: f32 = 8.;
const SPACING: f32 = 12.;

#[derive(Clone, Debug, PartialEq)]
pub enum LoginAction {
    Continue,
    Back,
    LogIn,
    OpenAccount(usize),
    AskDeleteAccount(usize),
    ConfirmDelete,
    CancelDelete,
    DismissNotice,
}

enum Stage {
    Server,
    Credentials(ServerInfo),
    Mfa(ServerInfo),
}

struct AccountRow {
    account: StoredAccount,
    mouse_state: MouseStateHandle,
    delete_button: button::Button,
}

pub struct LoginView {
    stage: Stage,
    server_field: ViewHandle<EditorView>,
    login_field: ViewHandle<EditorView>,
    password_field: ViewHandle<EditorView>,
    mfa_field: ViewHandle<EditorView>,
    busy: bool,
    error: Option<String>,
    /// Bumped on every request and on Back, so a late answer to an abandoned request is
    /// ignored.
    request: u64,
    accounts: Vec<AccountRow>,
    /// The account whose data is about to be deleted (asking for confirmation).
    deleting: Option<usize>,
    continue_button: button::Button,
    back_button: button::Button,
    login_button: button::Button,
    confirm_delete_button: button::Button,
    cancel_delete_button: button::Button,
    dismiss_notice_button: button::Button,
}

impl LoginView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::StateChanged => me.reset(ctx),
            SessionEvent::StatusChanged => {
                me.sync_accounts(ctx);
                ctx.notify();
            }
            _ => {}
        });
        let server_field = field("chat.example.com", false, ctx);
        // As in the webapp, these fields are labelled by their placeholder (an empty one would
        // also collapse the empty field).
        let login_field = field("Email or Username", false, ctx);
        let password_field = field("Password", true, ctx);
        let mfa_field = field("MFA code", false, ctx);
        for (handle, action) in [
            (&server_field, LoginAction::Continue),
            (&login_field, LoginAction::LogIn),
            (&password_field, LoginAction::LogIn),
            (&mfa_field, LoginAction::LogIn),
        ] {
            ctx.subscribe_to_view(handle, move |me, _, event, ctx| match event {
                EditorEvent::Enter => me.handle_action(&action, ctx),
                EditorEvent::Navigate(NavigationKey::Tab | NavigationKey::ShiftTab) => {
                    me.toggle_credentials_focus(ctx)
                }
                EditorEvent::Escape if !matches!(me.stage, Stage::Server) => {
                    me.handle_action(&LoginAction::Back, ctx)
                }
                _ => {}
            });
        }
        let mut view = Self {
            stage: Stage::Server,
            server_field,
            login_field,
            password_field,
            mfa_field,
            busy: false,
            error: None,
            request: 0,
            accounts: Vec::new(),
            deleting: None,
            continue_button: Default::default(),
            back_button: Default::default(),
            login_button: Default::default(),
            confirm_delete_button: Default::default(),
            cancel_delete_button: Default::default(),
            dismiss_notice_button: Default::default(),
        };
        view.reset(ctx);
        view
    }

    /// Back to the server stage with the session's prefill (after a logout, at launch).
    fn reset(&mut self, ctx: &mut ViewContext<Self>) {
        self.request += 1;
        self.busy = false;
        self.error = None;
        self.deleting = None;
        self.stage = Stage::Server;
        self.sync_accounts(ctx);
        let prefill = match Session::as_ref(ctx).state() {
            SessionState::SignedOut(signed_out) => signed_out.clone(),
            SessionState::Active(_) => Default::default(),
        };
        if let Some(server) = prefill.server {
            set_text(&self.server_field, &server, ctx);
        }
        if let Some(login_id) = prefill.login_id {
            set_text(&self.login_field, &login_id, ctx);
        }
        clear(&self.password_field, ctx);
        clear(&self.mfa_field, ctx);
        ctx.focus(&self.server_field);
        ctx.notify();
    }

    fn sync_accounts(&mut self, ctx: &mut ViewContext<Self>) {
        let accounts = Session::as_ref(ctx).accounts();
        let unchanged = accounts.len() == self.accounts.len()
            && accounts
                .iter()
                .zip(&self.accounts)
                .all(|(account, row)| *account == row.account);
        if unchanged {
            return;
        }
        self.deleting = None;
        self.accounts = accounts
            .iter()
            .map(|account| AccountRow {
                account: account.clone(),
                mouse_state: MouseStateHandle::default(),
                delete_button: Default::default(),
            })
            .collect();
    }

    pub fn focus(&self, ctx: &mut ViewContext<Self>) {
        let field = match self.stage {
            Stage::Server => &self.server_field,
            Stage::Credentials(_) if text(&self.login_field, ctx).is_empty() => &self.login_field,
            Stage::Credentials(_) => &self.password_field,
            Stage::Mfa(_) => &self.mfa_field,
        };
        ctx.focus(field);
    }

    fn toggle_credentials_focus(&mut self, ctx: &mut ViewContext<Self>) {
        if let Stage::Credentials(_) = self.stage {
            if self.login_field.as_ref(ctx).is_focused() {
                ctx.focus(&self.password_field);
            } else {
                ctx.focus(&self.login_field);
            }
        }
    }

    fn probe(&mut self, input: String, ctx: &mut ViewContext<Self>) {
        self.busy = true;
        self.error = None;
        self.request += 1;
        let request = self.request;
        let future = Session::as_ref(ctx).auth().probe(&input);
        ctx.spawn(future, move |me, result, ctx| {
            if me.request != request {
                return;
            }
            me.busy = false;
            match result {
                Ok(info) => {
                    // Show the URL that answered (with the scheme it needed).
                    set_text(&me.server_field, info.url.as_str(), ctx);
                    let placeholder = info.sign_in.login_placeholder();
                    me.login_field.update(ctx, |editor, ctx| {
                        editor.set_placeholder_text(placeholder, ctx)
                    });
                    if !info.sign_in.password() {
                        me.error = Some(no_password_login(&info));
                    }
                    me.stage = Stage::Credentials(info);
                    me.focus(ctx);
                }
                Err(error) => me.error = Some(error.message),
            }
            ctx.notify();
        });
        ctx.notify();
    }

    fn log_in(&mut self, ctx: &mut ViewContext<Self>) {
        let info = match &self.stage {
            Stage::Credentials(info) | Stage::Mfa(info) => info.clone(),
            Stage::Server => return,
        };
        let request = LoginRequest {
            login_id: text(&self.login_field, ctx).trim().to_owned(),
            password: text(&self.password_field, ctx),
            mfa_code: match self.stage {
                Stage::Mfa(_) => text(&self.mfa_field, ctx).trim().to_owned(),
                _ => String::new(),
            },
            ..LoginRequest::default()
        };
        if request.login_id.is_empty() || request.password.is_empty() {
            self.error = Some("Enter your login and password.".to_owned());
            ctx.notify();
            return;
        }
        if matches!(self.stage, Stage::Mfa(_)) && request.mfa_code.is_empty() {
            self.error = Some("Enter the MFA code.".to_owned());
            ctx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        self.request += 1;
        let id = self.request;
        let server = info.url.clone();
        let future = Session::as_ref(ctx).auth().login(server.clone(), request);
        ctx.spawn(future, move |me, result, ctx| {
            if me.request != id {
                return;
            }
            me.busy = false;
            match result {
                Ok(response) => {
                    clear(&me.password_field, ctx);
                    clear(&me.mfa_field, ctx);
                    Session::handle(ctx)
                        .update(ctx, |session, ctx| session.sign_in(server, response, ctx));
                }
                Err(AuthError { message, needs_mfa }) => {
                    if needs_mfa && !matches!(me.stage, Stage::Mfa(_)) {
                        me.stage = Stage::Mfa(info);
                        // The first answer only says a code is needed.
                        me.error = None;
                    } else {
                        // Retyping is easier from an empty field.
                        if needs_mfa {
                            clear(&me.mfa_field, ctx);
                        } else {
                            clear(&me.password_field, ctx);
                        }
                        me.error = Some(message);
                    }
                    me.focus(ctx);
                }
            }
            ctx.notify();
        });
        ctx.notify();
    }

    fn open_account(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(account) = self.accounts.get(index).map(|row| row.account.clone()) else {
            return;
        };
        let opened = Session::handle(ctx)
            .update(ctx, |session, ctx| session.continue_account(&account, ctx));
        if !opened {
            // No saved login: check the server and ask for the password.
            let server = account.server_url.clone().unwrap_or_default();
            set_text(&self.server_field, &server, ctx);
            set_text(&self.login_field, &account.user.username, ctx);
            self.probe(server, ctx);
        }
    }

    fn render_field(
        &self,
        label: Option<&str>,
        field: &ViewHandle<EditorView>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        if let Some(label) = label {
            column.add_child(
                Container::new(
                    Text::new_inline(label.to_owned(), appearance.ui_font_family(), 12.)
                        .with_color(theme.sub_text_color(theme.surface_1()).into_solid())
                        .finish(),
                )
                .with_margin_bottom(4.)
                .finish(),
            );
        }
        column
            .with_child(
                Container::new(ChildView::new(field).finish())
                    .with_uniform_padding(FIELD_PADDING)
                    .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                    .finish(),
            )
            .finish()
    }

    fn render_button(
        &self,
        button: &button::Button,
        label: &'static str,
        primary: bool,
        action: LoginAction,
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
                    disabled: self.busy,
                    on_click: Some(Box::new(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone())
                    })),
                    ..button::Options::default(appearance)
                },
            },
        )
    }

    fn render_server_stage(&self, appearance: &Appearance) -> Vec<Box<dyn Element>> {
        let mut children = vec![
            self.render_field(Some("Server URL"), &self.server_field, appearance),
            self.render_actions(
                vec![self.render_button(
                    &self.continue_button,
                    "Continue",
                    true,
                    LoginAction::Continue,
                    appearance,
                )],
                appearance,
            ),
        ];
        if !self.accounts.is_empty() {
            children.push(self.render_accounts(appearance));
        }
        children
    }

    fn render_credentials_stage(
        &self,
        info: &ServerInfo,
        appearance: &Appearance,
    ) -> Vec<Box<dyn Element>> {
        let mut children = vec![self.render_server_line(info, appearance)];
        if info.sign_in.password() {
            children.push(self.render_field(None, &self.login_field, appearance));
            children.push(self.render_field(None, &self.password_field, appearance));
        }
        let mut buttons = vec![self.render_button(
            &self.back_button,
            "Back",
            false,
            LoginAction::Back,
            appearance,
        )];
        if info.sign_in.password() {
            buttons.push(self.render_button(
                &self.login_button,
                "Log in",
                true,
                LoginAction::LogIn,
                appearance,
            ));
        }
        children.push(self.render_actions(buttons, appearance));
        children
    }

    fn render_mfa_stage(
        &self,
        info: &ServerInfo,
        appearance: &Appearance,
    ) -> Vec<Box<dyn Element>> {
        vec![
            self.render_server_line(info, appearance),
            self.render_text(
                "Enter the code from your authenticator app.".to_owned(),
                false,
                appearance,
            ),
            self.render_field(None, &self.mfa_field, appearance),
            self.render_actions(
                vec![
                    self.render_button(
                        &self.back_button,
                        "Back",
                        false,
                        LoginAction::Back,
                        appearance,
                    ),
                    self.render_button(
                        &self.login_button,
                        "Verify",
                        true,
                        LoginAction::LogIn,
                        appearance,
                    ),
                ],
                appearance,
            ),
        ]
    }

    fn render_server_line(&self, info: &ServerInfo, appearance: &Appearance) -> Box<dyn Element> {
        let mut line = format!("{} · {}", info.site_name, info.url.as_str());
        if let Some(version) = &info.version {
            line.push_str(&format!(
                " · Mattermost {}.{}.{}",
                version.major, version.minor, version.patch
            ));
        }
        self.render_text(line, true, appearance)
    }

    fn render_text(&self, text: String, dim: bool, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let color = if dim {
            theme.sub_text_color(theme.surface_1())
        } else {
            theme.main_text_color(theme.surface_1())
        };
        Text::new(text, appearance.ui_font_family(), appearance.ui_font_size())
            .with_color(color.into_solid())
            .finish()
    }

    fn render_actions(
        &self,
        buttons: Vec<Box<dyn Element>>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.);
        if self.busy {
            row.add_child(
                Shrinkable::new(
                    1.,
                    self.render_text("Connecting…".to_owned(), true, appearance),
                )
                .finish(),
            );
        }
        row.with_children(buttons).finish()
    }

    fn render_accounts(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new_inline("On this computer".to_owned(), font, 12.)
                        .with_color(theme.sub_text_color(theme.surface_1()).into_solid())
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .finish(),
                )
                .with_margin_top(SPACING)
                .with_margin_bottom(4.)
                .finish(),
            );
        let now = now_ms();
        for (index, row) in self.accounts.iter().enumerate() {
            let account = &row.account;
            if self.deleting == Some(index) {
                column.add_child(self.render_delete_confirmation(account, appearance));
                continue;
            }
            let title = format!(
                "{} · {}",
                account.user.username,
                account
                    .server_url
                    .as_deref()
                    .map(strip_scheme)
                    .unwrap_or("unknown server")
            );
            let mut details = format_size(account.size);
            if let Some(last_used) = account.last_used {
                details.push_str(" · ");
                details.push_str(&format_last_used(last_used, now));
            }
            if account.unsent > 0 {
                details.push_str(&format!(" · {} unsent", account.unsent));
            }
            let main = theme.main_text_color(theme.surface_1()).into_solid();
            let sub = theme.sub_text_color(theme.surface_1()).into_solid();
            let hover_fill = theme.surface_2();
            let busy = self.busy;
            let text = Flex::column()
                .with_child(
                    Text::new_inline(title, font, appearance.ui_font_size())
                        .with_color(main)
                        .finish(),
                )
                .with_child(
                    Text::new_inline(details, font, 11.)
                        .with_color(sub)
                        .finish(),
                )
                .finish();
            let open = Hoverable::new(row.mouse_state.clone(), move |state| {
                let mut container = Container::new(text)
                    .with_uniform_padding(6.)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
                if state.is_hovered() && !busy {
                    container = container.with_background(hover_fill);
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(LoginAction::OpenAccount(index)))
            .finish();
            let delete = row.delete_button.render(
                appearance,
                button::Params {
                    content: button::Content::Label("Delete…".into()),
                    theme: &button::themes::Naked,
                    options: button::Options {
                        disabled: self.busy,
                        size: button::Size::Small,
                        on_click: Some(Box::new(move |ctx, _, _| {
                            ctx.dispatch_typed_action(LoginAction::AskDeleteAccount(index))
                        })),
                        ..button::Options::default(appearance)
                    },
                },
            );
            column.add_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(Expanded::new(1., open).finish())
                    .with_child(delete)
                    .finish(),
            );
        }
        column.finish()
    }

    fn render_delete_confirmation(
        &self,
        account: &StoredAccount,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut message = format!(
            "Delete the local data of {} ({})? Messages stay on the server.",
            account.user.username,
            format_size(account.size)
        );
        if account.unsent > 0 {
            message.push_str(&format!(
                " {} unsent post{} will be lost.",
                account.unsent,
                if account.unsent == 1 { "" } else { "s" }
            ));
        }
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.)
                .with_child(self.render_text(message, false, appearance))
                .with_child(self.render_actions(
                    vec![
                        self.render_button(
                            &self.cancel_delete_button,
                            "Cancel",
                            false,
                            LoginAction::CancelDelete,
                            appearance,
                        ),
                        self.render_button(
                            &self.confirm_delete_button,
                            "Delete",
                            true,
                            LoginAction::ConfirmDelete,
                            appearance,
                        ),
                    ],
                    appearance,
                ))
                .finish(),
        )
        .with_uniform_padding(8.)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish()
    }

    fn render_notice(&self, notice: &str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let dismiss = self.dismiss_notice_button.render(
            appearance,
            button::Params {
                content: button::Content::Icon(warp_core::ui::Icon::X),
                theme: &button::themes::Naked,
                options: button::Options {
                    size: button::Size::Small,
                    on_click: Some(Box::new(|ctx, _, _| {
                        ctx.dispatch_typed_action(LoginAction::DismissNotice)
                    })),
                    ..button::Options::default(appearance)
                },
            },
        );
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Text::new(notice.to_owned(), appearance.ui_font_family(), 13.)
                            .with_color(theme.main_text_color(theme.surface_2()).into_solid())
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(dismiss)
                .finish(),
        )
        .with_uniform_padding(8.)
        .with_background(theme.surface_2())
        .with_border(Border::left(3.).with_border_color(theme.ui_warning_color()))
        .finish()
    }
}

fn field(
    placeholder: &str,
    password: bool,
    ctx: &mut ViewContext<LoginView>,
) -> ViewHandle<EditorView> {
    let options = SingleLineEditorOptions {
        text: TextOptions::ui_text(Some(FIELD_FONT_SIZE), Appearance::as_ref(ctx)),
        propagate_and_no_op_vertical_navigation_keys: PropagateAndNoOpNavigationKeys::Always,
        is_password: password,
        ..Default::default()
    };
    let editor = ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx));
    let placeholder = placeholder.to_owned();
    editor.update(ctx, |editor, ctx| {
        editor.set_placeholder_text(placeholder, ctx)
    });
    editor
}

fn text(field: &ViewHandle<EditorView>, ctx: &AppContext) -> String {
    field.as_ref(ctx).buffer_text(ctx)
}

fn set_text(field: &ViewHandle<EditorView>, text: &str, ctx: &mut ViewContext<LoginView>) {
    let text = text.to_owned();
    field.update(ctx, |editor, ctx| editor.set_buffer_text(&text, ctx));
}

fn clear(field: &ViewHandle<EditorView>, ctx: &mut ViewContext<LoginView>) {
    field.update(ctx, |editor, ctx| editor.system_clear_buffer(false, ctx));
}

fn no_password_login(info: &ServerInfo) -> String {
    if info.sign_in.sso.is_empty() {
        "This server doesn't allow logging in with a password.".to_owned()
    } else {
        format!(
            "This server only offers single sign-on ({}), which MatterLite doesn't support yet.",
            info.sign_in.sso.join(", ")
        )
    }
}

fn strip_scheme(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl Entity for LoginView {
    type Event = ();
}

impl View for LoginView {
    fn ui_name() -> &'static str {
        "LoginView"
    }

    fn on_focus(&mut self, focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focus(ctx);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let card_fill: Fill = theme.surface_1();
        let mut card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(SPACING)
            .with_child(
                Text::new_inline("MatterLite".to_owned(), appearance.ui_font_family(), 22.)
                    .with_color(theme.main_text_color(card_fill).into_solid())
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            );
        if let SessionState::SignedOut(signed_out) = Session::as_ref(app).state()
            && let Some(notice) = &signed_out.notice
        {
            card.add_child(self.render_notice(notice, appearance));
        }
        let stage = match &self.stage {
            Stage::Server => self.render_server_stage(appearance),
            Stage::Credentials(info) => self.render_credentials_stage(info, appearance),
            Stage::Mfa(info) => self.render_mfa_stage(info, appearance),
        };
        // The error goes right above the buttons.
        let actions_at = stage.len().min(match self.stage {
            Stage::Server => 1,
            _ => stage.len() - 1,
        });
        for (index, child) in stage.into_iter().enumerate() {
            if index == actions_at
                && let Some(error) = &self.error
            {
                card.add_child(
                    Text::new(error.clone(), appearance.ui_font_family(), 13.)
                        .with_color(theme.ui_error_color())
                        .finish(),
                );
            }
            card.add_child(child);
        }

        let card = Container::new(
            ConstrainedBox::new(card.finish())
                .with_width(CARD_WIDTH)
                .finish(),
        )
        .with_uniform_padding(24.)
        .with_background(card_fill)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .finish();
        Container::new(Align::new(card).finish())
            .with_background(theme.background())
            .finish()
    }
}

impl TypedActionView for LoginView {
    type Action = LoginAction;

    fn handle_action(&mut self, action: &LoginAction, ctx: &mut ViewContext<Self>) {
        if self.busy && !matches!(action, LoginAction::Back | LoginAction::DismissNotice) {
            return;
        }
        match action {
            LoginAction::Continue => {
                let input = text(&self.server_field, ctx);
                self.probe(input, ctx);
            }
            LoginAction::Back => {
                self.request += 1;
                self.busy = false;
                self.error = None;
                self.stage = match std::mem::replace(&mut self.stage, Stage::Server) {
                    Stage::Mfa(info) => Stage::Credentials(info),
                    Stage::Credentials(_) | Stage::Server => Stage::Server,
                };
                clear(&self.mfa_field, ctx);
                self.focus(ctx);
                ctx.notify();
            }
            LoginAction::LogIn => self.log_in(ctx),
            LoginAction::OpenAccount(index) => self.open_account(*index, ctx),
            LoginAction::AskDeleteAccount(index) => {
                self.deleting = Some(*index);
                ctx.notify();
            }
            LoginAction::CancelDelete => {
                self.deleting = None;
                ctx.notify();
            }
            LoginAction::ConfirmDelete => {
                if let Some(dir) = self
                    .deleting
                    .take()
                    .and_then(|index| self.accounts.get(index))
                    .map(|row| row.account.dir.clone())
                {
                    delete_account(dir, ctx);
                }
                ctx.notify();
            }
            LoginAction::DismissNotice => {
                Session::handle(ctx).update(ctx, |session, ctx| session.dismiss_notice(ctx));
            }
        }
    }
}

fn delete_account(dir: PathBuf, ctx: &mut ViewContext<LoginView>) {
    Session::handle(ctx).update(ctx, |session, ctx| session.delete_account(&dir, ctx));
}
