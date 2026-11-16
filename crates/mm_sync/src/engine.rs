// SPDX-License-Identifier: AGPL-3.0-only

//! The engine loop: startup, then commands, WebSocket outputs and timers.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_channel::{Receiver, Sender};
use mm_api::{ApiError, Client, ClientOptions, NewPost, PostsQuery};
use mm_cache::{Cache, Op, OutboxEntry, Writer, account_id, kv, metadata_ops, ops_for};
use mm_model::ws::{Broadcast, WsEvent};
use mm_model::{CategoryId, ChannelId, ChannelType, Post, ServerVersion, TeamId, User, UserId};
use mm_store::{PageRequest, StoreDelta, dm_partner};
use mm_ws::{ResumeState, WsConfig, WsHandle, WsOutput};
use serde_json::json;
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep, timeout};

use crate::bootstrap::{self, PAGE};
use crate::images::{self, ImageKey};
use crate::limiter::Limiter;
use crate::scheduler::{self, Context, Job, Outcome, Priority, Queue, USER_BATCH};
use crate::{
    Command, ConnectionState, EngineConfig, Event, History, ServerChange, SharedStore, SyncStatus,
    UI_STATE_PREFIX,
};

/// How long a resumed connection may take to be confirmed before the engine bootstraps anyway.
const RESUME_WAIT: Duration = Duration::from_secs(10);
/// Unconfirmed WebSocket connections in a row before the token is checked over REST (the
/// server accepts any token on the upgrade and drops the socket later, WebApp.md §7.2).
const UNCONFIRMED_BEFORE_CHECK: u32 = 3;
/// A failing job is tried this often before it's dropped.
const JOB_ATTEMPTS: u32 = 3;
/// `Event::Sync` at most this often (and when the sync goes idle).
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// The server recognises a resend by `pending_post_id` only this long
/// (`pendingPostIDsCacheTTL` in `server/channels/app/post.go`).
const PENDING_DEDUPE_MS: i64 = 30_000;

/// Results of work done off the loop.
enum Internal {
    PostSent {
        pending: String,
        result: Result<Box<Post>, ApiError>,
    },
    JobDone {
        priority: Priority,
        job: Job,
        result: Result<Outcome, ApiError>,
    },
    ImageSaved {
        key: ImageKey,
        result: Result<(), String>,
    },
    Stats(Result<mm_model::ChannelStats, ApiError>),
}

struct Engine {
    config: EngineConfig,
    client: Client,
    store: SharedStore,
    events: Sender<Event>,
    writer: Option<Writer>,
    ws: Option<WsHandle>,
    internal_tx: Sender<Internal>,
    /// The channel on screen and whether the window has focus.
    open_channel: Option<ChannelId>,
    focused: bool,
    online: bool,
    unconfirmed: u32,
    config_hash: Option<String>,
    /// DM/GM show preferences already sent this session.
    show_preferences_sent: HashSet<(String, String)>,
    /// Posts being sent or failed, as in the cache's outbox.
    unsent: HashMap<String, OutboxEntry>,
    /// `<servers_dir>/<account_id>`, once the cache is open.
    account_dir: Option<PathBuf>,
    /// `Some(delete_data)` after a logout command.
    logout: Option<bool>,
    /// The last saved `synced_through` (ms).
    last_saved_sync: i64,
    /// The `seq` of the last event the engine applied (or dropped as covered by a sync), so
    /// the saved resume state never skips an event that was received but not yet applied.
    applied_seq: Option<i64>,
    // The eager sync.
    limiter: Limiter,
    queue: Queue,
    running: usize,
    jobs_done: usize,
    job_attempts: HashMap<String, u32>,
    sync_paused: bool,
    last_status: Instant,
    /// When the last `since` sweep (or the catch-up) ran.
    last_sweep: i64,
    /// When profiles were last refreshed (`POST /users/ids?since=`).
    users_synced_at: Option<i64>,
    /// Images asked for this session (downloading, saved or failed): each is fetched once.
    images: HashSet<ImageKey>,
    stopping: bool,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

pub(crate) async fn run(
    config: EngineConfig,
    store: SharedStore,
    commands: Receiver<Command>,
    events: Sender<Event>,
) {
    let client = match Client::new(
        config.server.clone(),
        ClientOptions {
            user_agent: config.user_agent.clone(),
            ..ClientOptions::default()
        },
    ) {
        Ok(client) => client.with_token(config.token.clone()),
        Err(error) => {
            let _ = events.send(Event::Error(error.to_string())).await;
            return;
        }
    };
    let (internal_tx, internal_rx) = async_channel::unbounded();
    let limiter = Limiter::new(config.sync.rate);
    let mut engine = Engine {
        config,
        client,
        store,
        events,
        writer: None,
        ws: None,
        internal_tx,
        open_channel: None,
        focused: true,
        online: false,
        unconfirmed: 0,
        config_hash: None,
        show_preferences_sent: HashSet::new(),
        unsent: HashMap::new(),
        account_dir: None,
        logout: None,
        last_saved_sync: 0,
        applied_seq: None,
        limiter,
        queue: Queue::default(),
        running: 0,
        jobs_done: 0,
        job_attempts: HashMap::new(),
        sync_paused: false,
        last_status: Instant::now(),
        last_sweep: now_ms(),
        users_synced_at: None,
        images: HashSet::new(),
        stopping: false,
    };
    let Some(ws_output) = engine.start(&commands).await else {
        engine.stop().await;
        return;
    };
    engine.main_loop(&commands, &ws_output, &internal_rx).await;
    engine.stop().await;
}

impl Engine {
    async fn emit(&self, event: Event) {
        let _ = self.events.send(event).await;
    }

    async fn emit_changes(&self, deltas: Vec<StoreDelta>) {
        if deltas.is_empty() {
            return;
        }
        if let Some(writer) = &self.writer {
            let mut ops = ops_for(&self.store.read().expect("store lock"), &deltas);
            if self.config.sync.former_retention == Some(Duration::ZERO) {
                ops.extend(deltas.iter().filter_map(|delta| match delta {
                    StoreDelta::ChannelRemoved(channel) => Some(Op::PurgeChannel(channel.clone())),
                    _ => None,
                }));
            }
            if let Err(error) = writer.send(ops) {
                log::error!("cache: {error}");
            }
        }
        self.emit(Event::Changed(deltas)).await;
    }

    /// Startup up to `Ready`. Returns the WebSocket outputs, or `None` when the engine must
    /// stop (logged out, shut down).
    async fn start(&mut self, commands: &Receiver<Command>) -> Option<Receiver<WsOutput>> {
        // 1. The cache, if the account is known: draw before any request.
        let mut cache = None;
        if let Some(user) = self.config.user_id.clone() {
            cache = self.open_cache(&user).await;
        }

        // 2. The token, retried while the network is down.
        let me = self.check_token(commands).await?;
        if self.store.read().expect("store lock").me != me.id {
            // Another user than the cached one, or no cache yet.
            *self.store.write().expect("store lock") = mm_store::Store::new(me.clone());
            cache = None;
        }
        if cache.is_none() {
            cache = self.open_cache(&me.id).await;
        }
        let mut resume = None;
        let mut synced_through = None;
        let mut diagnostic_id = None;
        if let Some(cache) = cache.as_mut() {
            resume = cache
                .get_kv(kv::WS_RESUME)
                .ok()
                .flatten()
                .and_then(|text| parse_resume(&text));
            synced_through = cache
                .get_kv(kv::SYNCED_THROUGH)
                .ok()
                .flatten()
                .and_then(|text| text.parse::<i64>().ok());
            diagnostic_id = cache.get_kv(kv::DIAGNOSTIC_ID).ok().flatten();
            self.users_synced_at = cache
                .get_kv(kv::USERS_SYNCED_AT)
                .ok()
                .flatten()
                .and_then(|text| text.parse::<i64>().ok());
        }

        // 3. A reset server: ask before syncing into the old data.
        let mut cache_kept = true;
        let diagnostic_id = match self.server_identity_changed(diagnostic_id.as_deref()).await {
            Some(true) => match self.ask_server_change(commands).await? {
                ServerChange::KeepCache => None,
                ServerChange::ArchiveAndStartFresh => {
                    drop(cache.take());
                    self.archive_account().await;
                    *self.store.write().expect("store lock") = mm_store::Store::new(me.clone());
                    cache = self.open_cache(&me.id).await;
                    cache_kept = false;
                    None
                }
            },
            _ => diagnostic_id,
        };
        if !cache_kept {
            resume = None;
            synced_through = None;
        }

        self.last_saved_sync = synced_through.unwrap_or(0);
        let mut outbox = Vec::new();
        let mut expired = Vec::new();
        if let Some(cache) = cache.as_mut() {
            outbox = cache.outbox().unwrap_or_else(|error| {
                log::error!("outbox: {error}");
                Vec::new()
            });
            if let Some(retention) = self.config.sync.former_retention {
                let before = now_ms() - retention.as_millis() as i64;
                expired = cache.expired_former_chats(before).unwrap_or_default();
            }
        }
        if let Some(cache) = cache {
            match cache.into_writer() {
                Ok(writer) => self.writer = Some(writer),
                Err(error) => log::error!("cache writer: {error}"),
            }
        }
        if let Some(writer) = &self.writer {
            let mut ops: Vec<Op> = expired.into_iter().map(Op::PurgeChannel).collect();
            ops.push(Op::Kv(kv::LAST_USED.into(), Some(now_ms().to_string())));
            let _ = writer.send(ops);
        }
        self.restore_outbox(outbox).await;

        // 3. Resume the saved event stream, or bootstrap.
        let has_cache = !self.store.read().expect("store lock").channels.is_empty();
        let (ws, output) = self.connect_ws(resume.filter(|_| has_cache));
        self.ws = Some(ws);
        self.emit(Event::Connection(ConnectionState::Connecting))
            .await;
        let mut resumed = false;
        if has_cache
            && cache_kept
            && self
                .ws
                .as_ref()
                .is_some_and(|ws| !ws.resume_state().connection_id.is_empty())
        {
            resumed = self.wait_for_resume(&output).await?;
        }
        if !resumed {
            self.full_sync(&output, synced_through).await?;
            self.check_server_identity(diagnostic_id).await;
        }
        self.emit(Event::Ready { resumed }).await;
        self.last_sweep = if resumed {
            self.last_saved_sync
        } else {
            now_ms()
        };
        self.plan_eager();
        self.resend_outbox().await;
        Some(output)
    }

    /// Compares the server's `DiagnosticId` with the cached one. `None` if either is unknown.
    async fn server_identity_changed(&self, cached: Option<&str>) -> Option<bool> {
        let cached = cached.filter(|id| !id.is_empty())?;
        self.limiter.acquire().await;
        let config = self.client.client_config().await.ok()?;
        let current = config.get("DiagnosticId").filter(|id| !id.is_empty())?;
        Some(current != cached)
    }

    /// Reports `ServerChanged` and waits for the answer (or a shutdown).
    async fn ask_server_change(&mut self, commands: &Receiver<Command>) -> Option<ServerChange> {
        self.emit(Event::ServerChanged).await;
        loop {
            match commands.recv().await {
                Ok(Command::ResolveServerChange(answer)) => return Some(answer),
                Ok(Command::Shutdown) | Err(_) => return None,
                Ok(Command::Logout { delete_data }) => {
                    self.logout = Some(delete_data);
                    let _ = self.client.logout().await;
                    self.emit(Event::LoggedOut).await;
                    return None;
                }
                // Nothing else can run before the cache question is settled.
                Ok(_) => {}
            }
        }
    }

    /// Moves the account directory aside as an archive.
    async fn archive_account(&mut self) {
        let Some(dir) = self.account_dir.take() else {
            return;
        };
        let seconds = now_ms() / 1000;
        let archive = PathBuf::from(format!("{}.archived-{seconds}", dir.display()));
        match std::fs::rename(&dir, &archive) {
            Ok(()) => log::info!("archived {} to {}", dir.display(), archive.display()),
            Err(error) => {
                log::error!("archiving {}: {error}", dir.display());
                self.emit(Event::Error(format!(
                    "could not archive the old data: {error}"
                )))
                .await;
            }
        }
    }

    /// Shows the posts that were being sent when the app stopped.
    async fn restore_outbox(&mut self, entries: Vec<OutboxEntry>) {
        let mut deltas = Vec::new();
        for entry in entries {
            let pending = pending_post(&entry, entry.first_attempt_at);
            let mut store = self.store.write().expect("store lock");
            deltas.extend(store.add_pending(pending));
            if entry.failed {
                deltas.extend(store.fail_pending(&entry.pending_post_id));
            }
            drop(store);
            self.unsent.insert(entry.pending_post_id.clone(), entry);
        }
        // Only to the UI: the posts are in the outbox, not the cache.
        if !deltas.is_empty() {
            self.emit(Event::Changed(deltas)).await;
        }
    }

    /// After a restart: a post tried less than 30 s ago is sent again with the same
    /// `pending_post_id` (the server drops the duplicate); an older one is looked up first,
    /// and if it isn't on the server it's shown as failed, never re-sent blindly (§8.7).
    async fn resend_outbox(&mut self) {
        let entries: Vec<OutboxEntry> = self
            .unsent
            .values()
            .filter(|entry| !entry.failed)
            .cloned()
            .collect();
        let now = now_ms();
        for mut entry in entries {
            if now - entry.last_attempt_at < PENDING_DEDUPE_MS {
                entry.last_attempt_at = now;
                self.save_outbox(&entry);
                self.unsent
                    .insert(entry.pending_post_id.clone(), entry.clone());
                self.spawn_send(&entry);
                continue;
            }
            match self.find_sent(&entry).await {
                Ok(Some(mut post)) => {
                    post.pending_post_id = entry.pending_post_id.clone();
                    self.unsent.remove(&entry.pending_post_id);
                    self.delete_outbox(&entry.pending_post_id);
                    let deltas = self.store.write().expect("store lock").confirm_post(&post);
                    self.emit_changes(deltas).await;
                }
                Ok(None) => self.mark_failed(entry).await,
                Err(error) => {
                    log::warn!("checking {}: {error}", entry.pending_post_id);
                    self.mark_failed(entry).await;
                }
            }
        }
    }

    /// The server's copy of an outbox post, matched by author, channel, thread and text.
    async fn find_sent(&self, entry: &OutboxEntry) -> Result<Option<Post>, ApiError> {
        let me = self.store.read().expect("store lock").me.clone();
        self.limiter.acquire().await;
        let list = self
            .client
            .channel_posts(
                &entry.channel_id,
                &PostsQuery {
                    since: Some(entry.first_attempt_at - 60_000),
                    ..PostsQuery::default()
                },
            )
            .await?;
        Ok(list.posts.into_values().find(|post| {
            post.user_id == me
                && post.root_id == entry.root_id
                && post.message == entry.message
                && post.file_ids == entry.file_ids
                && !post.is_deleted()
        }))
    }

    async fn mark_failed(&mut self, mut entry: OutboxEntry) {
        entry.failed = true;
        self.save_outbox(&entry);
        let deltas = self
            .store
            .write()
            .expect("store lock")
            .fail_pending(&entry.pending_post_id);
        self.unsent.insert(entry.pending_post_id.clone(), entry);
        self.emit(Event::Changed(deltas)).await;
    }

    fn save_outbox(&self, entry: &OutboxEntry) {
        if let Some(writer) = &self.writer {
            let _ = writer.send(vec![Op::PutOutbox(entry.clone())]);
        }
    }

    fn delete_outbox(&self, pending: &str) {
        if let Some(writer) = &self.writer {
            let _ = writer.send(vec![Op::DeleteOutbox(pending.to_owned())]);
        }
    }

    async fn open_cache(&mut self, user: &UserId) -> Option<Cache> {
        let path = self
            .config
            .servers_dir
            .join(account_id(self.config.server.as_str(), user))
            .join("cache.sqlite");
        match Cache::open(&path) {
            Ok((mut cache, report)) => {
                if let Some(aside) = report.moved_aside {
                    self.emit(Event::CacheMovedAside(aside)).await;
                }
                if let Some(dir) = path.parent() {
                    self.account_dir = Some(dir.to_owned());
                    self.emit(Event::CacheOpened(dir.to_owned())).await;
                }
                match cache.hydrate() {
                    Ok(Some(hydrated)) if hydrated.me == *user => {
                        *self.store.write().expect("store lock") = hydrated;
                        self.emit(Event::Hydrated).await;
                    }
                    Ok(_) => {}
                    Err(error) => log::error!("cache hydration: {error}"),
                }
                Some(cache)
            }
            Err(error) => {
                log::error!("cache {}: {error}", path.display());
                self.emit(Event::Error(format!("local cache unavailable: {error}")))
                    .await;
                None
            }
        }
    }

    /// `GET /users/me` until it answers. 401: logged out.
    async fn check_token(&mut self, commands: &Receiver<Command>) -> Option<User> {
        let mut delay = Duration::from_secs(3);
        loop {
            match self.client.me().await {
                Ok(me) => return Some(me),
                Err(error) if error.is_session_expired() => {
                    self.emit(Event::LoggedOut).await;
                    return None;
                }
                Err(error) => {
                    log::warn!("server unreachable: {error}");
                    self.emit(Event::Connection(ConnectionState::Offline {
                        retry_in: delay,
                    }))
                    .await;
                    tokio::select! {
                        _ = sleep(delay) => {}
                        command = commands.recv() => match command {
                            Ok(Command::Shutdown) | Err(_) => return None,
                            // Nothing can be done offline before the first contact; the UI
                            // reads the hydrated store meanwhile.
                            Ok(_) => {}
                        },
                    }
                    delay = (delay * 2).min(Duration::from_secs(60));
                }
            }
        }
    }

    fn connect_ws(&self, resume: Option<ResumeState>) -> (WsHandle, Receiver<WsOutput>) {
        let url = {
            let store = self.store.read().expect("store lock");
            self.config
                .server
                .websocket_url(store.config.websocket_url())
        };
        let url = url.unwrap_or_else(|_| {
            self.config
                .server
                .websocket_url(None)
                .expect("a valid server URL has a WebSocket URL")
        });
        let mut ws_config = WsConfig::new(
            url,
            self.config.token.clone(),
            self.config.user_agent.clone(),
        );
        ws_config.resume = resume;
        mm_ws::connect(ws_config)
    }

    /// After a restart with a saved session: waits for the server to confirm it. `true` if
    /// it resumed with nothing lost.
    async fn wait_for_resume(&mut self, output: &Receiver<WsOutput>) -> Option<bool> {
        let deadline = Instant::now() + RESUME_WAIT;
        loop {
            let next = timeout(
                deadline.saturating_duration_since(Instant::now()),
                output.recv(),
            )
            .await;
            match next {
                Ok(Ok(WsOutput::Confirmed { resumed })) => {
                    self.set_online(true).await;
                    return Some(resumed);
                }
                Ok(Ok(WsOutput::MissedEvents)) => return Some(false),
                Ok(Ok(WsOutput::AuthFailed)) => {
                    self.emit(Event::LoggedOut).await;
                    return None;
                }
                Ok(Ok(WsOutput::Closed)) | Ok(Err(_)) => return None,
                Ok(Ok(WsOutput::Disconnected { .. })) | Err(_) => return Some(false),
                Ok(Ok(_)) => {}
            }
        }
    }

    /// The P0 bootstrap and the catch-up of loaded timelines. WebSocket events that arrive
    /// meanwhile are dropped: the fetched data already contains them, and applying them on
    /// top would count new posts twice. The catch-up covers posts in that window.
    async fn full_sync(
        &mut self,
        output: &Receiver<WsOutput>,
        synced_through: Option<i64>,
    ) -> Option<()> {
        let started = now_ms();
        loop {
            match bootstrap::bootstrap(&self.client, &self.limiter, &self.store).await {
                Ok(()) => break,
                Err(error) if error.is_session_expired() => {
                    self.emit(Event::LoggedOut).await;
                    return None;
                }
                Err(error) => {
                    log::warn!("bootstrap failed: {error}");
                    self.emit(Event::Error(format!("sync failed: {error}")))
                        .await;
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
        self.config_hash = self.client.version().map(|version| version.config_hash);
        if let Some(writer) = &self.writer {
            let ops = metadata_ops(&self.store.read().expect("store lock"));
            if let Err(error) = writer.send(ops) {
                log::error!("cache: {error}");
            }
        }
        let since = synced_through.unwrap_or(started);
        match bootstrap::catch_up(&self.client, &self.limiter, &self.store, since).await {
            Ok(deltas) => self.emit_changes(deltas).await,
            Err(error) => log::warn!("catch-up failed: {error}"),
        }
        let dropped = self.drop_queued_events(output).await?;
        if dropped > 0 {
            log::info!("dropped {dropped} events received during the sync");
            // Posts among them: covered by a second, short catch-up.
            if let Ok(deltas) =
                bootstrap::catch_up(&self.client, &self.limiter, &self.store, started).await
            {
                self.emit_changes(deltas).await;
            }
        }
        self.save_show_preferences().await;
        self.save_sync_time(started);
        self.emit_changes(vec![StoreDelta::PreferencesChanged])
            .await;
        Some(())
    }

    /// Discards queued events, handling the connection outputs among them.
    async fn drop_queued_events(&mut self, output: &Receiver<WsOutput>) -> Option<usize> {
        let mut dropped = 0;
        while let Ok(next) = output.try_recv() {
            match next {
                WsOutput::Event { seq, .. } | WsOutput::Undecodable { seq, .. } => {
                    dropped += 1;
                    self.applied_seq = Some(seq);
                }
                other => {
                    if !self.on_connection_output(other).await {
                        return None;
                    }
                }
            }
        }
        Some(dropped)
    }

    async fn main_loop(
        &mut self,
        commands: &Receiver<Command>,
        output: &Receiver<WsOutput>,
        internal: &Receiver<Internal>,
    ) {
        // The first poll right away: statuses aren't cached, and a resumed session brings none.
        let mut statuses = interval_at(Instant::now(), self.config.status_interval);
        statuses.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut save = interval_at(
            Instant::now() + self.config.save_interval,
            self.config.save_interval,
        );
        save.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut sweep = interval_at(
            Instant::now() + self.config.sync.sweep_interval,
            self.config.sync.sweep_interval,
        );
        sweep.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while !self.stopping {
            tokio::select! {
                command = commands.recv() => match command {
                    Ok(command) => self.on_command(command).await,
                    Err(_) => break,
                },
                next = output.recv() => match next {
                    Ok(WsOutput::Event { seq, broadcast, event }) => {
                        self.on_event(event, broadcast).await;
                        self.applied_seq = Some(seq);
                    }
                    Ok(WsOutput::Undecodable { seq, error }) => {
                        log::warn!("event {seq} not decoded: {error}");
                        self.applied_seq = Some(seq);
                    }
                    Ok(WsOutput::MissedEvents) => {
                        // The server lost our session: refetch, as at startup. The new
                        // connection's numbering starts over.
                        self.applied_seq = None;
                        let synced = self.synced_through();
                        if self.full_sync(output, Some(synced)).await.is_none() {
                            break;
                        }
                        self.emit(Event::Resynced).await;
                        // Edits in quiet channels during the outage, and anything the eager
                        // sync still lacks.
                        self.plan_sweep();
                        self.plan_eager();
                    }
                    Ok(other) => {
                        if !self.on_connection_output(other).await {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(message) = internal.recv() => self.on_internal(message).await,
                _ = statuses.tick() => {
                    self.poll_statuses().await;
                    self.check_config_hash(None).await;
                }
                _ = save.tick() => {
                    if self.online {
                        self.save_sync_time(now_ms());
                    }
                }
                _ = sweep.tick() => {
                    if !self.sync_paused {
                        self.plan_sweep();
                    }
                }
            }
        }
    }

    /// Connection outputs (not events). Returns `false` when the engine must stop.
    async fn on_connection_output(&mut self, output: WsOutput) -> bool {
        match output {
            WsOutput::Connected { .. } => {
                self.emit(Event::Connection(ConnectionState::Connecting))
                    .await;
            }
            WsOutput::Confirmed { .. } => self.set_online(true).await,
            WsOutput::Disconnected {
                retry_in,
                session_confirmed,
                ..
            } => {
                if self.online {
                    self.save_sync_time(now_ms());
                }
                self.online = false;
                self.emit(Event::Connection(ConnectionState::Offline { retry_in }))
                    .await;
                if session_confirmed {
                    self.unconfirmed = 0;
                } else {
                    self.unconfirmed += 1;
                    if self.unconfirmed >= UNCONFIRMED_BEFORE_CHECK {
                        self.unconfirmed = 0;
                        if let Err(error) = self.client.me().await
                            && error.is_session_expired()
                        {
                            self.emit(Event::LoggedOut).await;
                            return false;
                        }
                    }
                }
            }
            WsOutput::AuthFailed => {
                self.emit(Event::LoggedOut).await;
                return false;
            }
            WsOutput::Closed => return false,
            WsOutput::MissedEvents
            | WsOutput::Event { .. }
            | WsOutput::Undecodable { .. }
            | WsOutput::Response(_) => {}
        }
        true
    }

    async fn set_online(&mut self, online: bool) {
        if self.online != online {
            self.online = online;
            if online {
                self.unconfirmed = 0;
                self.emit(Event::Connection(ConnectionState::Online)).await;
            }
        }
    }

    async fn on_event(&mut self, event: WsEvent, broadcast: Broadcast) {
        if let WsEvent::Hello(hello) = &event {
            let hash =
                ServerVersion::parse(&hello.server_version).map(|version| version.config_hash);
            self.check_config_hash(hash).await;
        }
        let deltas = self
            .store
            .write()
            .expect("store lock")
            .apply(&event, &broadcast);
        let mut fetches = Vec::new();
        for delta in &deltas {
            match delta {
                StoreDelta::Fetch(fetch) => fetches.push(fetch.clone()),
                StoreDelta::MarkedRead {
                    channel,
                    notify_server: true,
                } => self.view_on_server(channel.clone()),
                _ => {}
            }
        }
        if let WsEvent::Posted(posted) = &event {
            let author = &posted.post.user_id;
            if !self
                .store
                .read()
                .expect("store lock")
                .users
                .contains_key(author)
            {
                fetches.push(mm_store::Fetch::Users(vec![author.clone()]));
            }
        }
        self.emit_changes(deltas).await;
        for fetch in fetches {
            self.fetch(fetch).await;
        }
        if matches!(event, WsEvent::Posted(_)) {
            self.save_show_preferences().await;
        }
    }

    async fn fetch(&mut self, fetch: mm_store::Fetch) {
        let result: Result<Vec<StoreDelta>, ApiError> = async {
            Ok(match fetch {
                mm_store::Fetch::Channel(id) => {
                    let (channel, member) = tokio::try_join!(
                        self.client.channel(&id),
                        self.client.my_channel_member(&id)
                    )?;
                    let partner = (channel.channel_type == ChannelType::Direct)
                        .then(|| {
                            let me = self.store.read().expect("store lock").me.clone();
                            dm_partner(&channel.name, &me)
                        })
                        .flatten();
                    let mut deltas = self
                        .store
                        .write()
                        .expect("store lock")
                        .upsert_channel(channel, member);
                    // Decide first: no store guard may live across an await.
                    let unknown = partner.filter(|partner| {
                        !self
                            .store
                            .read()
                            .expect("store lock")
                            .users
                            .contains_key(partner)
                    });
                    if let Some(partner) = unknown {
                        let users = self.client.users_by_ids(&[partner]).await?;
                        deltas.extend(self.store.write().expect("store lock").add_users(users));
                    }
                    deltas
                }
                mm_store::Fetch::Team(_)
                | mm_store::Fetch::Categories(_)
                | mm_store::Fetch::Roles => {
                    // Rare: a full bootstrap brings teams, channels, categories and roles
                    // consistently.
                    bootstrap::bootstrap(&self.client, &self.limiter, &self.store).await?;
                    let mut deltas = vec![StoreDelta::RolesChanged, StoreDelta::PreferencesChanged];
                    let store = self.store.read().expect("store lock");
                    deltas.extend(store.teams.keys().cloned().map(StoreDelta::TeamChanged));
                    deltas.extend(
                        store
                            .categories
                            .keys()
                            .cloned()
                            .map(StoreDelta::CategoriesChanged),
                    );
                    deltas.extend(
                        store
                            .channels
                            .keys()
                            .cloned()
                            .map(StoreDelta::ChannelChanged),
                    );
                    deltas.extend(
                        store
                            .channel_members
                            .keys()
                            .cloned()
                            .map(StoreDelta::MemberChanged),
                    );
                    deltas
                }
                mm_store::Fetch::Users(ids) => {
                    for batch in ids.chunks(USER_BATCH) {
                        self.enqueue(Priority::Interactive, Job::Users(batch.to_vec()));
                    }
                    Vec::new()
                }
            })
        }
        .await;
        match result {
            Ok(deltas) => self.emit_changes(deltas).await,
            Err(error) => log::warn!("fetch failed: {error}"),
        }
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::OpenChannel(channel) => self.open_channel(channel).await,
            Command::CloseChannel => {
                self.open_channel = None;
                self.store.write().expect("store lock").viewing = None;
            }
            Command::SetFocused(focused) => {
                self.focused = focused;
                let open = self.open_channel.clone();
                self.store.write().expect("store lock").viewing = open.clone().filter(|_| focused);
                if focused && let Some(channel) = open {
                    self.mark_viewed(&channel).await;
                }
            }
            Command::MarkRead(channel) => {
                self.store
                    .write()
                    .expect("store lock")
                    .manually_unread
                    .remove(&channel);
                self.mark_viewed(&channel).await;
            }
            Command::FetchImage(key) => self.fetch_image(key).await,
            Command::LoadOlder(channel) => self.load_older(channel),
            Command::LoadNewer(channel) => self.load_newer(channel),
            Command::OpenThread(root) => self.enqueue(Priority::Interactive, Job::Thread(root)),
            Command::PauseSync => {
                self.sync_paused = true;
                self.emit_sync_status(true).await;
            }
            Command::ResumeSync => {
                self.sync_paused = false;
                self.pump();
                self.emit_sync_status(true).await;
            }
            Command::SyncNow => {
                self.plan_sweep();
                self.plan_eager();
                self.emit_sync_status(true).await;
            }
            Command::SendPost {
                channel,
                root,
                message,
            } => {
                let me = self.store.read().expect("store lock").me.clone();
                let now = now_ms();
                let entry = OutboxEntry {
                    pending_post_id: format!("{me}:{now}"),
                    channel_id: channel,
                    root_id: root,
                    message,
                    props: Default::default(),
                    file_ids: Vec::new(),
                    first_attempt_at: now,
                    last_attempt_at: now,
                    failed: false,
                };
                self.send_post(entry).await;
            }
            Command::RetryPost(pending) => {
                if let Some(mut entry) = self.unsent.get(&pending).cloned() {
                    entry.failed = false;
                    entry.last_attempt_at = now_ms();
                    self.save_outbox(&entry);
                    self.unsent.insert(pending.clone(), entry.clone());
                    if let Some(state) = self
                        .store
                        .write()
                        .expect("store lock")
                        .pending
                        .get_mut(&pending)
                    {
                        state.failed = false;
                    }
                    self.emit_changes(vec![StoreDelta::PendingChanged(pending.clone())])
                        .await;
                    self.spawn_send(&entry);
                }
            }
            Command::DiscardPost(pending) => {
                self.unsent.remove(&pending);
                self.delete_outbox(&pending);
                let deltas = self
                    .store
                    .write()
                    .expect("store lock")
                    .discard_pending(&pending);
                self.emit_changes(deltas).await;
            }
            Command::ResolveServerChange(_) => {}
            Command::SetCategoryCollapsed {
                team,
                category,
                collapsed,
            } => self.set_category_collapsed(team, category, collapsed).await,
            Command::SetUiState { key, value } => {
                if let Some(writer) = &self.writer
                    && let Err(error) =
                        writer.send(vec![Op::Kv(format!("{UI_STATE_PREFIX}{key}"), value)])
                {
                    log::error!("cache: {error}");
                }
            }
            Command::Logout { delete_data } => {
                self.limiter.acquire().await;
                if let Err(error) = self.client.logout().await {
                    log::warn!("logout: {error}");
                }
                self.logout = Some(delete_data);
                self.emit(Event::LoggedOut).await;
                self.stopping = true;
            }
            Command::Shutdown => self.stopping = true,
        }
    }

    /// Collapses or expands a category at once, then saves it. If the server refuses, its
    /// categories are fetched again so the sidebar shows what it has.
    async fn set_category_collapsed(
        &mut self,
        team: TeamId,
        category: CategoryId,
        collapsed: bool,
    ) {
        let updated = {
            let mut store = self.store.write().expect("store lock");
            let Some(entry) = store
                .categories
                .get_mut(&team)
                .and_then(|categories| categories.iter_mut().find(|c| c.id == category))
            else {
                return;
            };
            if entry.collapsed == collapsed {
                return;
            }
            entry.collapsed = collapsed;
            entry.clone()
        };
        self.emit_changes(vec![StoreDelta::CategoriesChanged(team.clone())])
            .await;
        self.limiter.acquire().await;
        if let Err(error) = self.client.update_sidebar_category(&updated).await {
            log::warn!("saving category {category}: {error}");
            self.emit(Event::Error(format!(
                "The category couldn't be saved on the server: {}",
                crate::auth::describe(&error)
            )))
            .await;
            let me = self.store.read().expect("store lock").me.clone();
            self.limiter.acquire().await;
            match self.client.sidebar_categories(&me, &team).await {
                Ok(categories) => {
                    self.store
                        .write()
                        .expect("store lock")
                        .set_categories(team.clone(), categories);
                    self.emit_changes(vec![StoreDelta::CategoriesChanged(team)])
                        .await;
                }
                Err(error) => log::warn!("categories of {team}: {error}"),
            }
        }
    }

    async fn open_channel(&mut self, channel: ChannelId) {
        let previous = self.open_channel.replace(channel.clone());
        let (needs_posts, unread, last_viewed_at) = {
            let mut store = self.store.write().expect("store lock");
            if let Some(previous) = &previous
                && *previous != channel
            {
                store.manually_unread.remove(previous);
            }
            store.viewing = self.focused.then(|| channel.clone());
            let needs_posts = store
                .timelines
                .get(&channel)
                .is_none_or(|timeline| !timeline.has_recent());
            let unread = store
                .channel_unread(&channel)
                .is_some_and(|unread| unread.messages > 0);
            let last_viewed_at = store
                .channel_members
                .get(&channel)
                .map_or(0, |member| member.last_viewed_at);
            (needs_posts, unread, last_viewed_at)
        };
        self.emit(Event::ChannelOpened {
            channel: channel.clone(),
            last_viewed_at,
        })
        .await;
        if needs_posts {
            self.enqueue(
                Priority::Interactive,
                Job::Open {
                    channel: channel.clone(),
                    unread,
                },
            );
        }
        if self.focused {
            self.mark_viewed(&channel).await;
        }
        self.fetch_stats(channel);
    }

    /// The member count for the channel header.
    fn fetch_stats(&self, channel: ChannelId) {
        let client = self.client.clone();
        let limiter = self.limiter.clone();
        let internal = self.internal_tx.clone();
        tokio::spawn(async move {
            limiter.acquire().await;
            let result = client.channel_stats(&channel).await;
            let _ = internal.send(Internal::Stats(result)).await;
        });
    }

    /// Downloads an image once per session; [`Event::ImageReady`] when it is on disk.
    async fn fetch_image(&mut self, key: ImageKey) {
        let Some(dir) = self.account_dir.clone() else {
            return;
        };
        if key.path(&dir).is_file() {
            self.emit(Event::ImageReady(key)).await;
            return;
        }
        if !self.images.insert(key.clone()) {
            return;
        }
        let client = self.client.clone();
        let limiter = self.limiter.clone();
        let internal = self.internal_tx.clone();
        tokio::spawn(async move {
            limiter.acquire().await;
            let bytes = match &key {
                ImageKey::Avatar { user, updated } => client.user_image(user, *updated).await,
                ImageKey::Emoji(id) => client.emoji_image(id).await,
            };
            let result = bytes.map_err(|error| error.to_string()).and_then(|bytes| {
                images::save(&dir, &key, &bytes).map_err(|error| error.to_string())
            });
            let _ = internal.send(Internal::ImageSaved { key, result }).await;
        });
    }

    async fn mark_viewed(&mut self, channel: &ChannelId) {
        if self
            .store
            .read()
            .expect("store lock")
            .manually_unread
            .contains(channel)
        {
            return;
        }
        let deltas = self
            .store
            .write()
            .expect("store lock")
            .mark_viewed(channel, now_ms());
        self.emit_changes(deltas).await;
        self.view_on_server(channel.clone());
    }

    fn view_on_server(&self, channel: ChannelId) {
        let client = self.client.clone();
        let limiter = self.limiter.clone();
        tokio::spawn(async move {
            limiter.acquire().await;
            if let Err(error) = client.view_channel(&channel, None).await {
                log::warn!("marking {channel} read: {error}");
            }
        });
    }

    fn load_older(&mut self, channel: ChannelId) {
        let anchor = self
            .store
            .read()
            .expect("store lock")
            .timelines
            .get(&channel)
            .and_then(|timeline| timeline.oldest_of_recent().cloned());
        if let Some((_, anchor)) = anchor {
            self.enqueue(Priority::Interactive, Job::Older { channel, anchor });
        }
    }

    fn load_newer(&mut self, channel: ChannelId) {
        let anchor = self
            .store
            .read()
            .expect("store lock")
            .timelines
            .get(&channel)
            .filter(|timeline| !timeline.has_recent())
            .and_then(|timeline| timeline.ranges().last().map(|range| range.newest.clone()));
        if let Some((_, anchor)) = anchor {
            self.enqueue(Priority::Interactive, Job::Newer { channel, anchor });
        }
    }

    /// Profiles of a page's authors that aren't loaded yet.
    fn fetch_missing_authors(&mut self, list: &mm_model::PostList, priority: Priority) {
        let missing: Vec<UserId> = {
            let store = self.store.read().expect("store lock");
            list.posts
                .values()
                .map(|post| post.user_id.clone())
                .filter(|id| !id.is_empty() && !store.users.contains_key(id))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        for batch in missing.chunks(USER_BATCH) {
            self.enqueue(priority.min(Priority::People), Job::Users(batch.to_vec()));
        }
    }

    // ---- the eager sync (MatterLite.md §8.5) ----

    fn enqueue(&mut self, priority: Priority, job: Job) {
        self.queue.push(priority, job);
        self.pump();
    }

    /// Starts queued jobs up to the concurrency limit; while paused, only interactive ones.
    fn pump(&mut self) {
        while self.running < self.config.sync.concurrency.max(1) {
            let next = if self.sync_paused {
                self.queue.pop_interactive()
            } else {
                self.queue.pop()
            };
            let Some((priority, job)) = next else {
                break;
            };
            self.running += 1;
            let context = {
                let store = self.store.read().expect("store lock");
                Context {
                    client: self.client.clone(),
                    limiter: self.limiter.clone(),
                    me: store.me.clone(),
                    crt: store.crt_enabled(),
                }
            };
            let internal = self.internal_tx.clone();
            tokio::spawn(async move {
                let result = scheduler::run(context, job.clone()).await;
                let _ = internal
                    .send(Internal::JobDone {
                        priority,
                        job,
                        result,
                    })
                    .await;
            });
        }
    }

    /// Queues everything the eager sync does from the current state: P1 people and emoji,
    /// P2 newest pages, P3 followed threads, P5 backfill. Progress is in the store's
    /// timelines (cached), so after a restart this continues where it stopped.
    fn plan_eager(&mut self) {
        let now = now_ms();
        let settings = self.config.sync.clone();
        let (recent, backfill, teams, crt) = {
            let store = self.store.read().expect("store lock");
            (
                scheduler::plan_recent(&store),
                scheduler::plan_backfill(&store, settings.history, now),
                store.team_members.keys().cloned().collect::<Vec<_>>(),
                store.crt_enabled(),
            )
        };
        if settings.people {
            self.queue.push(Priority::People, Job::KnownUsers);
            if let Some(since) = self.users_synced_at {
                self.queue.push(
                    Priority::People,
                    Job::ModifiedUsers {
                        since: since - bootstrap::SINCE_MARGIN,
                    },
                );
            }
            self.queue.push(Priority::People, Job::Emoji { page: 0 });
        }
        for channel in recent {
            self.queue.push(Priority::Recent, Job::Newest(channel));
        }
        if crt && settings.history != History::Off {
            for team in teams {
                self.queue
                    .push(Priority::Threads, Job::Threads { team, before: None });
            }
        }
        for (channel, anchor) in backfill {
            self.queue
                .push(Priority::Backfill, Job::Backfill { channel, anchor });
        }
        self.pump();
    }

    /// Queues a `since` request for every loaded channel: edits, deletes and reactions in
    /// quiet channels don't move `last_post_at`, so the catch-up after a resync misses them.
    fn plan_sweep(&mut self) {
        let since = self.last_sweep - bootstrap::SINCE_MARGIN;
        self.last_sweep = now_ms();
        let channels: Vec<ChannelId> = {
            let store = self.store.read().expect("store lock");
            scheduler::channel_order(&store)
                .into_iter()
                .filter(|channel| {
                    store
                        .timelines
                        .get(channel)
                        .is_some_and(|timeline| timeline.has_recent())
                })
                .collect()
        };
        for channel in channels {
            self.queue
                .push(Priority::Sweep, Job::Sweep { channel, since });
        }
        self.pump();
    }

    async fn on_job_done(
        &mut self,
        priority: Priority,
        job: Job,
        result: Result<Outcome, ApiError>,
    ) {
        self.running -= 1;
        self.queue.finished(&job);
        match result {
            Ok(outcome) => {
                self.jobs_done += 1;
                self.job_attempts.remove(&format!("{job:?}"));
                if matches!(job, Job::KnownUsers | Job::ModifiedUsers { .. }) {
                    let now = now_ms();
                    self.users_synced_at = Some(now);
                    if let Some(writer) = &self.writer {
                        let _ = writer.send(vec![Op::Kv(
                            kv::USERS_SYNCED_AT.into(),
                            Some(now.to_string()),
                        )]);
                    }
                }
                self.apply_outcome(priority, &job, outcome).await;
            }
            Err(ApiError::RateLimited { retry_after }) => {
                log::warn!("rate limited; pausing requests for {retry_after:?}");
                self.limiter.pause(retry_after).await;
                self.queue.retry(priority, job);
            }
            Err(error) if error.is_session_expired() => {
                self.emit(Event::LoggedOut).await;
                self.stopping = true;
            }
            Err(error) => {
                let status = match &error {
                    ApiError::App(app) => Some(app.status_code),
                    ApiError::Status { status, .. } => Some(*status),
                    _ => None,
                };
                // 4xx: the request itself is wrong for this channel (no permission, gone);
                // anything else (network, 5xx): try again later, a few times.
                let permanent = status.is_some_and(|status| (400..500).contains(&status));
                let key = format!("{job:?}");
                let attempts = self.job_attempts.entry(key.clone()).or_default();
                *attempts += 1;
                if permanent || *attempts >= JOB_ATTEMPTS {
                    self.job_attempts.remove(&key);
                    log::warn!("giving up on {job:?}: {error}");
                    if priority == Priority::Interactive {
                        self.emit(Event::Error(format!("loading failed: {error}")))
                            .await;
                    }
                } else {
                    log::info!("{job:?} failed ({error}); queued again");
                    self.queue.push(priority, job);
                }
            }
        }
        self.pump();
        let idle = self.running == 0 && self.queue.len() == 0;
        self.emit_sync_status(idle).await;
    }

    async fn apply_outcome(&mut self, priority: Priority, job: &Job, outcome: Outcome) {
        let now = now_ms();
        let history = self.config.sync.history;
        match outcome {
            Outcome::Page {
                channel,
                list,
                request,
            } => {
                let member = self
                    .store
                    .read()
                    .expect("store lock")
                    .channel_members
                    .contains_key(&channel);
                if !member {
                    return; // Left the channel meanwhile.
                }
                let since_incomplete = matches!(request, PageRequest::Since { complete: false });
                let deltas = self
                    .store
                    .write()
                    .expect("store lock")
                    .add_posts(&channel, &list, request);
                self.emit_changes(deltas).await;
                self.fetch_missing_authors(&list, priority);
                if since_incomplete {
                    self.enqueue(Priority::Recent, Job::Newest(channel.clone()));
                }
                // After the newest page, and after each backfill page: the next backfill
                // page, queued at the back so channels take turns.
                if matches!(
                    job,
                    Job::Newest(_) | Job::Open { .. } | Job::Backfill { .. }
                ) {
                    let anchor = scheduler::backfill_anchor(
                        &self.store.read().expect("store lock"),
                        &channel,
                        history,
                        now,
                    );
                    if let Some(anchor) = anchor {
                        self.queue
                            .push(Priority::Backfill, Job::Backfill { channel, anchor });
                    }
                }
            }
            Outcome::Thread { root, list } => {
                let deltas = self
                    .store
                    .write()
                    .expect("store lock")
                    .add_thread(&root, &list);
                self.emit_changes(deltas).await;
                self.fetch_missing_authors(&list, priority);
            }
            Outcome::Threads { team, list } => {
                let to_load = scheduler::threads_to_load(
                    &self.store.read().expect("store lock"),
                    &list,
                    history,
                    now,
                );
                let next = (list.threads.len() as u32 >= scheduler::THREAD_PAGE)
                    .then(|| list.threads.last().map(|thread| thread.id.clone()))
                    .flatten();
                let deltas = self
                    .store
                    .write()
                    .expect("store lock")
                    .set_threads(&team, list);
                self.emit_changes(deltas).await;
                for root in to_load {
                    self.queue.push(Priority::Threads, Job::Thread(root));
                }
                if let Some(before) = next {
                    self.queue.push(
                        Priority::Threads,
                        Job::Threads {
                            team,
                            before: Some(before),
                        },
                    );
                }
            }
            Outcome::KnownUsers(ids) => {
                let missing: Vec<UserId> = {
                    let store = self.store.read().expect("store lock");
                    ids.into_iter()
                        .filter(|id| !store.users.contains_key(id))
                        .collect()
                };
                for batch in missing.chunks(USER_BATCH) {
                    self.queue
                        .push(Priority::People, Job::Users(batch.to_vec()));
                }
            }
            Outcome::Users(users) => {
                let deltas = self.store.write().expect("store lock").add_users(users);
                self.emit_changes(deltas).await;
            }
            Outcome::Emoji { page, list } => {
                let full = list.len() as u32 >= scheduler::EMOJI_PAGE;
                self.store.write().expect("store lock").add_emoji(list);
                self.emit_changes(vec![StoreDelta::EmojiChanged]).await;
                if full {
                    self.queue
                        .push(Priority::People, Job::Emoji { page: page + 1 });
                }
            }
        }
    }

    async fn emit_sync_status(&mut self, force: bool) {
        if !force && self.last_status.elapsed() < STATUS_EVERY {
            return;
        }
        self.last_status = Instant::now();
        let backfilling = {
            let store = self.store.read().expect("store lock");
            let now = now_ms();
            scheduler::channel_order(&store)
                .iter()
                .filter(|channel| {
                    scheduler::backfill_anchor(&store, channel, self.config.sync.history, now)
                        .is_some()
                })
                .count()
        };
        self.emit(Event::Sync(SyncStatus {
            queued: self.queue.len(),
            running: self.running,
            done: self.jobs_done,
            backfilling,
            paused: self.sync_paused,
        }))
        .await;
    }

    async fn send_post(&mut self, entry: OutboxEntry) {
        let pending = pending_post(&entry, entry.first_attempt_at);
        let deltas = self.store.write().expect("store lock").add_pending(pending);
        self.emit_changes(deltas).await;
        // Saved before the request, so a crash in between can't lose the post.
        self.save_outbox(&entry);
        self.unsent
            .insert(entry.pending_post_id.clone(), entry.clone());
        self.spawn_send(&entry);
    }

    fn spawn_send(&self, entry: &OutboxEntry) {
        let client = self.client.clone();
        let limiter = self.limiter.clone();
        let internal = self.internal_tx.clone();
        let post = NewPost {
            channel_id: entry.channel_id.clone(),
            root_id: entry.root_id.clone(),
            message: entry.message.clone(),
            pending_post_id: entry.pending_post_id.clone(),
            file_ids: entry.file_ids.clone(),
            props: entry.props.clone(),
        };
        tokio::spawn(async move {
            limiter.acquire().await;
            let result = client.create_post(&post).await.map(Box::new);
            let _ = internal
                .send(Internal::PostSent {
                    pending: post.pending_post_id,
                    result,
                })
                .await;
        });
    }

    async fn on_internal(&mut self, message: Internal) {
        match message {
            Internal::JobDone {
                priority,
                job,
                result,
            } => self.on_job_done(priority, job, result).await,
            Internal::ImageSaved { key, result } => match result {
                Ok(()) => self.emit(Event::ImageReady(key)).await,
                Err(error) => log::debug!("image {key:?}: {error}"),
            },
            Internal::Stats(result) => match result {
                Ok(stats) => {
                    let deltas = self
                        .store
                        .write()
                        .expect("store lock")
                        .set_channel_stats(stats);
                    self.emit_changes(deltas).await;
                }
                Err(error) => log::debug!("channel stats: {error}"),
            },
            Internal::PostSent { pending, result } => match result {
                Ok(post) => {
                    self.unsent.remove(&pending);
                    self.delete_outbox(&pending);
                    let deltas = self.store.write().expect("store lock").confirm_post(&post);
                    self.emit_changes(deltas).await;
                }
                Err(error) => {
                    log::warn!("sending {pending}: {error}");
                    if let Some(entry) = self.unsent.get_mut(&pending) {
                        entry.failed = true;
                        let entry = entry.clone();
                        self.save_outbox(&entry);
                    }
                    let deltas = self
                        .store
                        .write()
                        .expect("store lock")
                        .fail_pending(&pending);
                    self.emit_changes(deltas).await;
                    self.emit(Event::Error(format!("message not sent: {error}")))
                        .await;
                }
            },
        }
    }

    /// Statuses of the users on screen: DM partners and the authors of the open channel's
    /// recent posts (`status_change` arrives only for my own status).
    async fn poll_statuses(&mut self) {
        let ids: Vec<UserId> = {
            let store = self.store.read().expect("store lock");
            let mut ids: BTreeSet<UserId> = store
                .channels
                .values()
                .filter(|channel| channel.channel_type == ChannelType::Direct)
                .filter_map(|channel| dm_partner(&channel.name, &store.me))
                .collect();
            if let Some(open) = &self.open_channel {
                for post in store
                    .timeline_posts(open)
                    .into_iter()
                    .flatten()
                    .take(PAGE as usize)
                {
                    ids.insert(post.user_id.clone());
                }
            }
            ids.insert(store.me.clone());
            ids.into_iter().filter(|id| !id.is_empty()).collect()
        };
        match self.client.statuses_by_ids(&ids).await {
            Ok(statuses) => {
                let deltas = self
                    .store
                    .write()
                    .expect("store lock")
                    .set_statuses(statuses);
                // Only to the UI: statuses aren't cached.
                self.emit(Event::Changed(deltas)).await;
            }
            Err(error) if error.is_session_expired() => {
                self.emit(Event::LoggedOut).await;
                self.stopping = true;
            }
            Err(error) => log::debug!("status poll: {error}"),
        }
    }

    /// Refetches the client config when the server's config hash changed (from `hello` or the
    /// last response's `X-Version-Id`).
    async fn check_config_hash(&mut self, from_hello: Option<String>) {
        let current =
            from_hello.or_else(|| self.client.version().map(|version| version.config_hash));
        let Some(current) = current.filter(|hash| !hash.is_empty()) else {
            return;
        };
        if self.config_hash.as_ref() == Some(&current) {
            return;
        }
        let first = self.config_hash.is_none();
        self.config_hash = Some(current);
        if first {
            return;
        }
        match tokio::try_join!(self.client.client_config(), self.client.client_license()) {
            Ok((config, license)) => {
                let crt_before = self.store.read().expect("store lock").crt_enabled();
                let mut deltas = vec![StoreDelta::ConfigChanged, StoreDelta::LicenseChanged];
                {
                    let mut store = self.store.write().expect("store lock");
                    store.set_config(config);
                    store.set_license(license);
                    if store.crt_enabled() != crt_before {
                        deltas.push(StoreDelta::CrtChanged);
                    }
                }
                self.emit_changes(deltas).await;
            }
            Err(error) => log::warn!("refetching the config: {error}"),
        }
    }

    /// Saves the `direct_channel_show`/`group_channel_show` preferences the webapp writes for
    /// unread DMs/GMs, so they stay in the sidebar once read (WebApp.md §14).
    async fn save_show_preferences(&mut self) {
        let (me, preferences) = {
            let store = self.store.read().expect("store lock");
            let preferences: Vec<_> = store
                .dm_gm_show_preferences_to_save()
                .into_iter()
                .filter(|p| {
                    !self
                        .show_preferences_sent
                        .contains(&(p.category.clone(), p.name.clone()))
                })
                .collect();
            (store.me.clone(), preferences)
        };
        if preferences.is_empty() {
            return;
        }
        match self.client.save_preferences(&me, &preferences).await {
            Ok(()) => {
                let mut store = self.store.write().expect("store lock");
                for preference in preferences {
                    self.show_preferences_sent
                        .insert((preference.category.clone(), preference.name.clone()));
                    store
                        .preferences
                        .insert((preference.category, preference.name), preference.value);
                }
            }
            Err(error) => log::warn!("saving DM visibility: {error}"),
        }
    }

    async fn check_server_identity(&mut self, cached: Option<String>) {
        let current = self
            .store
            .read()
            .expect("store lock")
            .config
            .get("DiagnosticId")
            .map(str::to_owned);
        if let (Some(cached), Some(current)) = (&cached, &current)
            && cached != current
        {
            self.emit(Event::ServerChanged).await;
        }
        if let (Some(writer), Some(current)) = (&self.writer, current) {
            let _ = writer.send(vec![
                Op::Kv(kv::DIAGNOSTIC_ID.into(), Some(current)),
                Op::Kv(
                    kv::SERVER_URL.into(),
                    Some(self.config.server.as_str().to_owned()),
                ),
            ]);
        }
    }

    /// Saved every `save_interval` while online and at each disconnect.
    fn synced_through(&self) -> i64 {
        self.last_saved_sync
    }

    fn save_sync_time(&mut self, at: i64) {
        self.last_saved_sync = at;
        if let Some(writer) = &self.writer {
            let mut ops = vec![Op::Kv(kv::SYNCED_THROUGH.into(), Some(at.to_string()))];
            if let Some(ws) = &self.ws {
                let mut resume = ws.resume_state();
                if let Some(seq) = self.applied_seq {
                    resume.sequence_number = seq + 1;
                }
                if !resume.connection_id.is_empty() {
                    ops.push(Op::Kv(
                        kv::WS_RESUME.into(),
                        Some(
                            json!({
                                "connection_id": resume.connection_id,
                                "sequence_number": resume.sequence_number,
                            })
                            .to_string(),
                        ),
                    ));
                }
            }
            let _ = writer.send(ops);
        }
    }

    async fn stop(&mut self) {
        if self.ws.is_some() {
            // The resume state too, also when offline: the server may still hold the session.
            let at = if self.online {
                now_ms()
            } else {
                self.last_saved_sync
            };
            self.save_sync_time(at);
        }
        if let Some(ws) = self.ws.take() {
            ws.close();
        }
        if let Some(writer) = self.writer.take()
            && let Err(error) = writer.close()
        {
            log::error!("closing the cache: {error}");
        }
        if self.logout == Some(true)
            && let Some(dir) = self.account_dir.take()
        {
            match mm_cache::delete_account(&dir) {
                Ok(()) => log::info!("deleted the local data in {}", dir.display()),
                Err(error) => {
                    self.emit(Event::Error(format!(
                        "could not delete the local data: {error}"
                    )))
                    .await;
                }
            }
        }
    }
}

fn parse_resume(text: &str) -> Option<ResumeState> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(ResumeState {
        connection_id: value.get("connection_id")?.as_str()?.to_owned(),
        sequence_number: value.get("sequence_number")?.as_i64()?,
    })
}

/// The pending post shown for an outbox entry.
fn pending_post(entry: &OutboxEntry, at: i64) -> Post {
    serde_json::from_value(json!({
        "channel_id": entry.channel_id,
        "root_id": entry.root_id,
        "message": entry.message,
        "props": entry.props,
        "file_ids": entry.file_ids,
        "pending_post_id": entry.pending_post_id,
        "create_at": at,
        "update_at": at,
    }))
    .expect("a post from known fields")
}
