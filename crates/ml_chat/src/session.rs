// SPDX-License-Identifier: AGPL-3.0-only

//! The signed-in account (MatterLite.md §6, §8.7, §9): a singleton model that owns the sync
//! engine and forwards its events on the UI thread.
//!
//! On start the most recently used account whose token is in secure storage opens straight
//! from its cache. Signing in stores the token (`token-<account id>`) and starts an engine.
//! When the engine reports `LoggedOut` (logout, or a revoked or expired session) the token is
//! removed and the session returns to the login screen, keeping the account's local data
//! unless the user asked to delete it.

use std::path::{Path, PathBuf};

use mm_api::{LoginResponse, ServerUrl};
use mm_cache::StoredAccount;
use mm_model::User;
use mm_store::StoreDelta;
use mm_sync::{
    Auth, Command, ConnectionState, EngineConfig, EngineHandle, Event, ServerChange, SharedStore,
    SyncStatus,
};
use warpui::{Entity, ModelContext, SingletonEntity};
use warpui_extras::secure_storage::AppContextExt as _;

/// The secure-storage key of an account's session token. No `:` in it: on Windows the key is
/// part of a file name.
pub fn token_key(account_id: &str) -> String {
    format!("token-{account_id}")
}

/// What the login screen shows when nobody is signed in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignedOut {
    /// Why the previous session ended, if it did on its own.
    pub notice: Option<String>,
    /// The server and login to fill in.
    pub server: Option<String>,
    pub login_id: Option<String>,
}

/// The running account.
pub struct Active {
    pub account_id: String,
    pub server: ServerUrl,
    pub user: User,
    engine: EngineHandle,
    /// The UI can draw: hydrated from the cache, or bootstrapped.
    pub hydrated: bool,
    /// Online and in sync with the server.
    pub ready: bool,
    pub connection: ConnectionState,
    pub sync: SyncStatus,
    /// The server was reset or reinstalled: the engine waits for
    /// [`Session::resolve_server_change`].
    pub server_changed: bool,
    pub logging_out: bool,
    /// Things to tell the user once (a damaged cache moved aside, a token that couldn't be
    /// saved).
    pub notices: Vec<String>,
    pub last_error: Option<String>,
}

pub enum SessionState {
    SignedOut(SignedOut),
    Active(Box<Active>),
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// Signed in or out.
    StateChanged,
    /// Connection, sync progress, readiness, notices or a pending question changed.
    StatusChanged,
    /// The store changed.
    Changed(Vec<StoreDelta>),
    /// The whole state was reloaded (bootstrap or resync): redraw everything.
    Reloaded,
    ChannelOpened {
        channel: mm_model::ChannelId,
        last_viewed_at: i64,
    },
    Error(String),
}

pub struct Session {
    auth: Auth,
    servers_dir: PathBuf,
    state: SessionState,
    accounts: Vec<StoredAccount>,
    /// Counts engines started, so events of a replaced engine are ignored.
    generation: u64,
}

impl Session {
    pub fn new(servers_dir: PathBuf) -> Self {
        Self {
            auth: Auth::new(mm_api::default_user_agent()),
            servers_dir,
            state: SessionState::SignedOut(SignedOut::default()),
            accounts: Vec::new(),
            generation: 0,
        }
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    pub fn active(&self) -> Option<&Active> {
        match &self.state {
            SessionState::Active(active) => Some(active),
            SessionState::SignedOut(_) => None,
        }
    }

    fn active_mut(&mut self) -> Option<&mut Active> {
        match &mut self.state {
            SessionState::Active(active) => Some(active),
            SessionState::SignedOut(_) => None,
        }
    }

    /// The shared store of the running account.
    pub fn store(&self) -> Option<&SharedStore> {
        self.active().map(|active| active.engine.store())
    }

    pub fn send(&self, command: Command) {
        if let Some(active) = self.active() {
            active.engine.send(command);
        }
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    /// Accounts with local data, most recently used first (as of the last sign-out).
    pub fn accounts(&self) -> &[StoredAccount] {
        &self.accounts
    }

    pub fn refresh_accounts(&mut self, ctx: &mut ModelContext<Self>) {
        self.accounts = mm_cache::stored_accounts(&self.servers_dir);
        ctx.emit(SessionEvent::StatusChanged);
    }

    /// At launch: opens the most recently used account if its token is stored, otherwise
    /// shows the login with that account filled in.
    pub fn restore(&mut self, ctx: &mut ModelContext<Self>) {
        self.accounts = mm_cache::stored_accounts(&self.servers_dir);
        let Some(account) = self.accounts.first().cloned() else {
            return;
        };
        if !self.continue_account(&account, ctx) {
            self.state = SessionState::SignedOut(prefill(&account, None));
            ctx.emit(SessionEvent::StateChanged);
        }
    }

    /// Opens a stored account with its saved token. False when there is no token (or no
    /// server URL), so the user must log in.
    pub fn continue_account(
        &mut self,
        account: &StoredAccount,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let Some(server) = account
            .server_url
            .as_deref()
            .and_then(|url| ServerUrl::parse(url).ok())
        else {
            return false;
        };
        let id = account_id_of(&account.dir);
        let token = match ctx.secure_storage().read_value(&token_key(&id)) {
            Ok(token) if !token.is_empty() => token,
            Ok(_) | Err(warpui_extras::secure_storage::Error::NotFound) => return false,
            Err(error) => {
                log::warn!("reading the stored login of {id}: {error}");
                return false;
            }
        };
        log::info!("opening account {id} on {}", server.as_str());
        self.start(
            id,
            server,
            account.user.clone(),
            mm_model::SessionToken::new(token),
            Vec::new(),
            ctx,
        );
        true
    }

    /// After a successful login: stores the token and starts the engine.
    pub fn sign_in(
        &mut self,
        server: ServerUrl,
        response: LoginResponse,
        ctx: &mut ModelContext<Self>,
    ) {
        let id = mm_cache::account_id(server.as_str(), &response.user.id);
        let mut notices = Vec::new();
        if let Err(error) = ctx
            .secure_storage()
            .write_value(&token_key(&id), response.token.expose())
        {
            log::warn!("saving the login of {id}: {error}");
            notices.push(
                "The login couldn't be saved in the system's secure storage, so you'll have to \
                 log in again next time."
                    .to_owned(),
            );
        }
        log::info!("signed in as account {id} on {}", server.as_str());
        self.start(id, server, response.user, response.token, notices, ctx);
    }

    /// Logs out (the engine revokes the session); the local data stays unless `delete_data`.
    pub fn log_out(&mut self, delete_data: bool, ctx: &mut ModelContext<Self>) {
        if let Some(active) = self.active_mut() {
            active.logging_out = true;
            active.engine.send(Command::Logout { delete_data });
            ctx.emit(SessionEvent::StatusChanged);
        }
    }

    pub fn resolve_server_change(&mut self, answer: ServerChange, ctx: &mut ModelContext<Self>) {
        if let Some(active) = self.active_mut()
            && active.server_changed
        {
            active.server_changed = false;
            active.engine.send(Command::ResolveServerChange(answer));
            ctx.emit(SessionEvent::StatusChanged);
        }
    }

    pub fn dismiss_notice(&mut self, ctx: &mut ModelContext<Self>) {
        match &mut self.state {
            SessionState::Active(active) => {
                if active.notices.is_empty() {
                    return;
                }
                active.notices.remove(0);
            }
            SessionState::SignedOut(signed_out) => {
                if signed_out.notice.take().is_none() {
                    return;
                }
            }
        }
        ctx.emit(SessionEvent::StatusChanged);
    }

    /// Deletes a stored account's local data and its saved token (login screen).
    pub fn delete_account(&mut self, dir: &Path, ctx: &mut ModelContext<Self>) {
        if self
            .active()
            .is_some_and(|active| self.servers_dir.join(&active.account_id) == dir)
        {
            return;
        }
        let id = account_id_of(dir);
        let _ = ctx.secure_storage().remove_value(&token_key(&id));
        if let Err(error) = mm_cache::delete_account(dir) {
            log::warn!("deleting {}: {error}", dir.display());
        }
        self.refresh_accounts(ctx);
    }

    /// At quit: stops the engine, saving its state and flushing the cache.
    pub fn shutdown(&mut self) {
        if let SessionState::Active(active) = std::mem::replace(
            &mut self.state,
            SessionState::SignedOut(SignedOut::default()),
        ) {
            active.engine.shutdown();
        }
    }

    fn start(
        &mut self,
        account_id: String,
        server: ServerUrl,
        user: User,
        token: mm_model::SessionToken,
        notices: Vec<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.stop_engine();
        let mut config = EngineConfig::new(server.clone(), token, self.servers_dir.clone());
        if self.servers_dir.join(&account_id).is_dir() {
            config.user_id = Some(user.id.clone());
        }
        let engine = mm_sync::start(config);
        self.generation += 1;
        let generation = self.generation;
        ctx.spawn_stream_local(
            engine.events().clone(),
            move |me, event, ctx| me.on_engine_event(generation, event, ctx),
            move |me, ctx| me.on_engine_gone(generation, ctx),
        );
        self.state = SessionState::Active(Box::new(Active {
            account_id,
            server,
            user,
            engine,
            hydrated: false,
            ready: false,
            connection: ConnectionState::Connecting,
            sync: SyncStatus::default(),
            server_changed: false,
            logging_out: false,
            notices,
            last_error: None,
        }));
        ctx.emit(SessionEvent::StateChanged);
    }

    /// Stops the running engine without waiting on the UI thread.
    fn stop_engine(&mut self) {
        if let SessionState::Active(active) = std::mem::replace(
            &mut self.state,
            SessionState::SignedOut(SignedOut::default()),
        ) {
            let engine = active.engine;
            let _ = std::thread::Builder::new()
                .name("engine shutdown".into())
                .spawn(move || engine.shutdown());
        }
    }

    /// Back to the login screen.
    fn end(&mut self, signed_out: SignedOut, ctx: &mut ModelContext<Self>) {
        self.stop_engine();
        self.accounts = mm_cache::stored_accounts(&self.servers_dir);
        self.state = SessionState::SignedOut(signed_out);
        ctx.emit(SessionEvent::StateChanged);
    }

    fn on_engine_event(&mut self, generation: u64, event: Event, ctx: &mut ModelContext<Self>) {
        if generation != self.generation {
            return;
        }
        let Some(active) = self.active_mut() else {
            return;
        };
        match event {
            Event::Hydrated => {
                active.hydrated = true;
                ctx.emit(SessionEvent::Reloaded);
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::CacheMovedAside(path) => {
                active.notices.push(format!(
                    "The local cache was damaged and has been moved aside ({}). It is being \
                     rebuilt from the server.",
                    path.display()
                ));
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::CacheOpened(path) => log::info!("cache {}", path.display()),
            Event::Ready { resumed } => {
                log::info!(
                    "ready ({})",
                    if resumed { "resumed" } else { "bootstrapped" }
                );
                active.hydrated = true;
                active.ready = true;
                if let Ok(store) = active.engine.store().read()
                    && let Some(me) = store.users.get(&store.me)
                {
                    active.user = User::clone(me);
                }
                if !resumed {
                    ctx.emit(SessionEvent::Reloaded);
                }
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::Resynced => ctx.emit(SessionEvent::Reloaded),
            Event::ServerChanged => {
                active.server_changed = true;
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::Changed(deltas) => ctx.emit(SessionEvent::Changed(deltas)),
            Event::ChannelOpened {
                channel,
                last_viewed_at,
            } => ctx.emit(SessionEvent::ChannelOpened {
                channel,
                last_viewed_at,
            }),
            Event::Connection(connection) => {
                active.connection = connection;
                if connection != ConnectionState::Online {
                    active.ready = false;
                }
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::Sync(status) => {
                active.sync = status;
                ctx.emit(SessionEvent::StatusChanged);
            }
            Event::LoggedOut => {
                let _ = ctx
                    .secure_storage()
                    .remove_value(&token_key(&active.account_id));
                let notice = (!active.logging_out).then(|| {
                    "Your session ended (it expired or was revoked). Log in again.".to_owned()
                });
                let signed_out = SignedOut {
                    notice,
                    server: Some(active.server.as_str().to_owned()),
                    login_id: Some(active.user.username.clone()),
                };
                log::info!("logged out of account {}", active.account_id);
                self.end(signed_out, ctx);
            }
            Event::Error(message) => {
                log::warn!("engine: {message}");
                active.last_error = Some(message.clone());
                ctx.emit(SessionEvent::Error(message));
            }
            Event::Stopped => self.on_engine_gone(generation, ctx),
        }
    }

    /// The engine stopped without a logout (a fatal error).
    fn on_engine_gone(&mut self, generation: u64, ctx: &mut ModelContext<Self>) {
        if generation != self.generation {
            return;
        }
        let Some(active) = self.active() else {
            // After a logout: the engine has now deleted the data if asked.
            self.refresh_accounts(ctx);
            return;
        };
        let mut notice = "The connection to the server stopped.".to_owned();
        if let Some(error) = &active.last_error {
            notice.push(' ');
            notice.push_str(error);
        }
        let signed_out = SignedOut {
            notice: Some(notice),
            server: Some(active.server.as_str().to_owned()),
            login_id: Some(active.user.username.clone()),
        };
        // A later engine must not be confused with this one.
        self.generation += 1;
        self.end(signed_out, ctx);
    }
}

/// The login fields for a stored account.
pub fn prefill(account: &StoredAccount, notice: Option<String>) -> SignedOut {
    SignedOut {
        notice,
        server: account.server_url.clone(),
        login_id: Some(account.user.username.clone()),
    }
}

/// An account directory is named after its account id.
fn account_id_of(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Entity for Session {
    type Event = SessionEvent;
}

impl SingletonEntity for Session {}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
