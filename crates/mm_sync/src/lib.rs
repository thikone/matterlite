// SPDX-License-Identifier: AGPL-3.0-only

//! The sync engine of one account (MatterLite.md §6, §8.2, §8.7).
//!
//! [`start`] runs the engine on its own thread with a Tokio runtime. It owns the REST client,
//! the WebSocket and the cache writer, and keeps the shared [`Store`] current. The UI sends
//! [`Command`]s and receives [`Event`]s; it reads the state through [`EngineHandle::store`].
//!
//! Startup: the cache is hydrated first (when the account is known), so the UI can draw at
//! once. The token is checked with `GET /users/me`. Then the saved WebSocket session is
//! resumed if the server still has it (nothing to fetch), else the P0 bootstrap runs and the
//! loaded timelines catch up with `since`. From then on events go through
//! [`Store::apply`] into the cache, and a lost event stream (`MissedEvents`) triggers the same
//! bootstrap and catch-up.

mod bootstrap;
mod engine;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use mm_api::ServerUrl;
use mm_model::{ChannelId, PostId, SessionToken, UserId};
use mm_store::{Store, StoreDelta};

pub use bootstrap::{SINCE_CAP, SINCE_MARGIN};

/// The state the UI reads. The engine holds the write lock only while applying one change.
pub type SharedStore = Arc<RwLock<Store>>;

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub server: ServerUrl,
    pub token: SessionToken,
    /// The account's user, when known from the stored accounts: the cache is then read
    /// before the first request.
    pub user_id: Option<UserId>,
    /// `state/servers/`; the cache goes to `<servers_dir>/<account_id>/cache.sqlite`.
    pub servers_dir: PathBuf,
    pub user_agent: String,
    /// How often statuses of visible users are polled (`status_change` covers only mine).
    pub status_interval: Duration,
    /// How often the WebSocket resume state and the sync time are saved.
    pub save_interval: Duration,
}

impl EngineConfig {
    pub fn new(server: ServerUrl, token: SessionToken, servers_dir: PathBuf) -> Self {
        Self {
            server,
            token,
            user_id: None,
            servers_dir,
            user_agent: mm_api::default_user_agent(),
            status_interval: Duration::from_secs(60),
            save_interval: Duration::from_secs(30),
        }
    }
}

/// What the UI asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The channel on screen: loads its posts if needed and marks it read while the window is
    /// focused. Answered with [`Event::ChannelOpened`].
    OpenChannel(ChannelId),
    /// No channel on screen.
    CloseChannel,
    /// The window gained or lost focus; posts in the open channel mark it read only while
    /// focused.
    SetFocused(bool),
    /// Loads the page before the oldest loaded post of the open stretch.
    LoadOlder(ChannelId),
    /// Loads the page after the newest loaded post, when the loaded stretch doesn't reach the
    /// newest post (after opening at the first unread post).
    LoadNewer(ChannelId),
    /// Loads a whole thread.
    OpenThread(PostId),
    SendPost {
        channel: ChannelId,
        /// Empty for a root post.
        root: PostId,
        message: String,
    },
    /// Sends a failed post again with the same `pending_post_id`.
    RetryPost(String),
    DiscardPost(String),
    /// Saves state, closes the WebSocket and the cache, and ends the engine.
    Shutdown,
}

/// What the engine reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The store was filled from the cache; it's shown as stale until [`Event::Ready`].
    Hydrated,
    /// The cache file was damaged and moved aside; it is refilled from the server.
    CacheMovedAside(PathBuf),
    /// The store is current: after the bootstrap, or `resumed` from the saved WebSocket
    /// session without a bootstrap.
    Ready {
        resumed: bool,
    },
    /// The server lost the event stream mid-session (restart, long outage); the bootstrap and
    /// catch-up ran again and the store is current.
    Resynced,
    /// The server's `DiagnosticId` differs from the cached one: it was reinstalled or reset
    /// (MatterLite.md §8.7). The cache is kept until the user decides.
    ServerChanged,
    Changed(Vec<StoreDelta>),
    /// `last_viewed_at` as it was when the channel was opened, for the "new messages" line.
    ChannelOpened {
        channel: ChannelId,
        last_viewed_at: i64,
    },
    Connection(ConnectionState),
    /// The session expired or was revoked: log in again. The data stays.
    LoggedOut,
    /// A command or a request failed.
    Error(String),
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline { retry_in: Duration },
}

/// Controls a running engine.
pub struct EngineHandle {
    commands: Sender<Command>,
    events: Receiver<Event>,
    store: SharedStore,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    /// Stops the engine and waits for it (state saved, cache flushed).
    pub fn shutdown(mut self) {
        let _ = self.commands.try_send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.commands.try_send(Command::Shutdown);
            let _ = thread.join();
        }
    }
}

/// Starts the engine on its own thread.
pub fn start(config: EngineConfig) -> EngineHandle {
    let (command_tx, command_rx) = async_channel::unbounded();
    let (event_tx, event_rx) = async_channel::unbounded();
    let store = SharedStore::default();
    let shared = store.clone();
    let thread = std::thread::Builder::new()
        .name("sync engine".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(engine::run(config, shared, command_rx, event_tx.clone()));
            let _ = event_tx.try_send(Event::Stopped);
        })
        .expect("sync engine thread");
    EngineHandle {
        commands: command_tx,
        events: event_rx,
        store,
        thread: Some(thread),
    }
}
