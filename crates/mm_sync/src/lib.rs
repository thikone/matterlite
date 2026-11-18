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

pub mod auth;
mod bootstrap;
mod engine;
pub mod images;
mod limiter;
mod scheduler;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use mm_api::ServerUrl;
use mm_model::{CategoryId, ChannelId, PostId, SessionToken, TeamId, UserId};

pub use mm_cache::UI_STATE_PREFIX;
use mm_store::{Store, StoreDelta};

pub use auth::{Auth, AuthError, ServerInfo, SignInOptions};
pub use bootstrap::{SINCE_CAP, SINCE_MARGIN};
pub use images::ImageKey;
pub use limiter::{Limiter, Rate};
pub use scheduler::Priority;

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
    pub sync: SyncSettings,
}

/// The eager sync (MatterLite.md §8.5, Settings → Offline & sync).
#[derive(Clone, Debug, PartialEq)]
pub struct SyncSettings {
    pub rate: Rate,
    /// Requests running at once.
    pub concurrency: usize,
    /// How far back history is fetched (and kept, §8.3).
    pub history: History,
    /// Profiles of everyone I share a channel with, and all custom emoji (P1).
    pub people: bool,
    /// How often quiet channels are checked for edits and deletes (`since`).
    pub sweep_interval: Duration,
    /// How long former chats (left, removed, archived) stay cached; `None` keeps them,
    /// `Some(ZERO)` deletes them at once.
    pub former_retention: Option<Duration>,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            rate: Rate::NORMAL,
            concurrency: 4,
            history: History::Days(180),
            people: true,
            sweep_interval: Duration::from_secs(30 * 60),
            former_retention: Some(Duration::from_secs(90 * 24 * 60 * 60)),
        }
    }
}

/// What to do with the cache of a server that was reset or reinstalled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerChange {
    /// Move the old data aside (`<dir>.archived-<unix seconds>`) and start an empty cache.
    ArchiveAndStartFresh,
    /// Merge into the old data (the server was restored from a backup, say).
    KeepCache,
}

/// The history backfill window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    /// The newest page of each channel only.
    Off,
    Days(u32),
    All,
}

/// Progress of the eager sync, for the status indicator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStatus {
    pub queued: usize,
    pub running: usize,
    /// Jobs finished this session.
    pub done: usize,
    /// Channels whose history doesn't cover the window yet.
    pub backfilling: usize,
    pub paused: bool,
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
            sync: SyncSettings::default(),
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
    /// Marks the channel read now, also after it was marked unread by hand (`Esc`).
    MarkRead(ChannelId),
    /// Downloads a profile picture or custom emoji image into the account's folder unless it
    /// is there; answered with [`Event::ImageReady`] (nothing if it fails). Once per session.
    FetchImage(ImageKey),
    /// Loads the page before the oldest loaded post of the open stretch.
    LoadOlder(ChannelId),
    /// Loads the page after the newest loaded post, when the loaded stretch doesn't reach the
    /// newest post (after opening at the first unread post).
    LoadNewer(ChannelId),
    /// Loads a whole thread.
    OpenThread(PostId),
    /// Marks a followed thread read now (CRT), as the webapp does while it is on screen.
    MarkThreadRead(PostId),
    /// Pins or unpins a post: shown at once, undone if the server refuses.
    PinPost {
        post: PostId,
        pinned: bool,
    },
    /// Saves a post, or removes it from Saved (the `flagged_post` preference).
    SavePost {
        post: PostId,
        saved: bool,
    },
    /// Marks the chat unread from a post on; with `in_thread` under CRT, the post's thread
    /// instead (the webapp's `markPostAsUnread` by location).
    MarkUnread {
        post: PostId,
        in_thread: bool,
    },
    /// Follows or unfollows a thread (CRT).
    FollowThread {
        root: PostId,
        follow: bool,
    },
    /// Deletes a post; the server's `post_deleted` removes it (with its replies for a root).
    DeletePost(PostId),
    /// Adds or removes my reaction; adding also moves the emoji up in my recently used
    /// ones (`recent_emojis`), as the webapp's `addReaction`.
    React {
        post: PostId,
        emoji: String,
        add: bool,
    },
    /// Loads the earlier versions of my post (`Store::edit_history`).
    LoadEditHistory(PostId),
    /// Gives my post an earlier version's message; the response updates the store.
    RestorePostVersion {
        post: PostId,
        version: PostId,
    },
    /// Changes a post's message (`PUT /posts/{id}/patch`); the response updates the store.
    EditPost {
        post: PostId,
        message: String,
    },
    SendPost {
        channel: ChannelId,
        /// Empty for a root post.
        root: PostId,
        message: String,
    },
    /// Moves a channel into a sidebar category of a team (Favorites, or back to Channels or
    /// Direct Messages), saved on the server as the webapp's `addChannelToCategory`.
    MoveToCategory {
        team: TeamId,
        channel: ChannelId,
        category: CategoryId,
    },
    /// Mutes or unmutes a channel for me (`mark_unread`).
    SetMuted {
        channel: ChannelId,
        muted: bool,
    },
    /// Leaves a channel; the server's events remove it.
    LeaveChannel(ChannelId),
    /// Loads a channel's members (`Store::channel_users`) and their statuses.
    LoadMembers(ChannelId),
    /// Loads a channel's pinned posts (`Store::pinned`).
    LoadPinned(ChannelId),
    /// Saves the unsent text of a composer in the cache (`message` empty: deletes it).
    SaveDraft {
        channel: ChannelId,
        /// Empty for the channel's composer.
        root: PostId,
        message: String,
    },
    /// Tells the server I'm typing (WebSocket `user_typing`); the caller throttles it to
    /// `TimeBetweenUserTypingUpdatesMilliseconds`. Dropped while offline.
    Typing {
        channel: ChannelId,
        root: PostId,
    },
    /// Sends a failed post again with the same `pending_post_id`.
    RetryPost(String),
    DiscardPost(String),
    /// Pauses the eager sync (user actions still run).
    PauseSync,
    ResumeSync,
    /// Plans the eager sync again now ("Sync now").
    SyncNow,
    /// Reconnects the WebSocket at once instead of waiting for the next retry ("Retry now").
    ReconnectNow,
    /// The answer to [`Event::ServerChanged`].
    ResolveServerChange(ServerChange),
    /// Collapses or expands a sidebar category; saved to the server as the webapp does, so
    /// other devices follow.
    SetCategoryCollapsed {
        team: TeamId,
        category: CategoryId,
        collapsed: bool,
    },
    /// Saves a piece of UI state in the account's cache (`kv`, key prefixed with
    /// [`UI_STATE_PREFIX`]); read back with [`mm_cache::ui_state`].
    SetUiState {
        key: String,
        value: Option<String>,
    },
    /// Logs out (`POST /users/logout`) and stops. The local data stays unless `delete_data`
    /// (MatterLite.md §8.7); the token is the caller's to forget.
    Logout {
        delete_data: bool,
    },
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
    /// (MatterLite.md §8.7). The engine waits for [`Command::ResolveServerChange`] before it
    /// syncs anything into the cache.
    ServerChanged,
    /// The account's data directory (`<servers_dir>/<account_id>`), for reading its cache
    /// (`mm_cache::open_reader`: search, former chats).
    CacheOpened(PathBuf),
    Changed(Vec<StoreDelta>),
    /// `last_viewed_at` as it was when the channel was opened, for the "new messages" line.
    ChannelOpened {
        channel: ChannelId,
        last_viewed_at: i64,
    },
    /// An image asked for with [`Command::FetchImage`] is at [`ImageKey::path`].
    ImageReady(ImageKey),
    Connection(ConnectionState),
    /// The eager sync's progress (at most every half second, and when it goes idle).
    Sync(SyncStatus),
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
