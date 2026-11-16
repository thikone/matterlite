// SPDX-License-Identifier: AGPL-3.0-only

//! The SQLite cache of one account on one server (MatterLite.md §8.3, §8.7).
//!
//! One file per account (`state/servers/<account_id>/cache.sqlite`), diesel over a bundled
//! SQLite with FTS5. On start the sync engine opens it, hydrates a `mm_store::Store` from it to
//! draw the first frame without the network, then hands the connection to the writer thread.
//! From then on every store change goes through [`ops_for`] into the [`Writer`], which commits
//! batches. Reads during the session (search) use [`open_reader`]. A broken file is moved
//! aside and rebuilt by the next sync ([`OpenReport`]).

mod account;
mod accounts;
mod db;
mod error;
mod hydrate;
mod lifecycle;
mod ops;
mod schema;
mod search;
mod writer;

use std::path::Path;

use diesel::sqlite::SqliteConnection;
use mm_model::ChannelId;
use mm_store::Store;

pub use account::account_id;
pub use accounts::{
    Draft, StoredAccount, UI_STATE_PREFIX, delete_account, delete_unused_accounts, drafts,
    stored_accounts, ui_state,
};
pub use db::{OpenReport, open_reader};
pub use error::CacheError;
pub use lifecycle::{FormerChat, OutboxEntry};
pub use ops::{Op, Owner, kv, metadata_ops, ops_for, snapshot_ops};
pub use search::{SearchHit, fts_query};
pub use writer::{BATCH_OPS, BATCH_WINDOW, Writer};

/// An open cache before the session starts: read it, then turn it into the [`Writer`].
pub struct Cache {
    conn: SqliteConnection,
}

impl Cache {
    /// Opens or creates the cache at `path` (MatterLite.md §8.3: WAL, migrations, broken files
    /// moved aside).
    pub fn open(path: &Path) -> Result<(Self, OpenReport), CacheError> {
        let (conn, report) = db::open(path)?;
        Ok((Self { conn }, report))
    }

    /// The cached state, or `None` if the cache was never filled.
    pub fn hydrate(&mut self) -> Result<Option<Store>, CacheError> {
        hydrate::hydrate(&mut self.conn)
    }

    pub fn get_kv(&mut self, key: &str) -> Result<Option<String>, CacheError> {
        hydrate::get_kv(&mut self.conn, key)
    }

    /// Writes ops at once, without the writer thread (setup and tests).
    pub fn apply(&mut self, ops: &[Op]) -> Result<(), CacheError> {
        db::apply(&mut self.conn, ops)
    }

    pub fn search(
        &mut self,
        query: &str,
        channel: Option<&ChannelId>,
        limit: i64,
    ) -> Result<Vec<SearchHit>, CacheError> {
        search::search(&mut self.conn, query, channel, limit)
    }

    /// Posts being sent when the app last stopped, oldest first.
    pub fn outbox(&mut self) -> Result<Vec<OutboxEntry>, CacheError> {
        lifecycle::outbox(&mut self.conn)
    }

    /// Chats I'm no longer a member of, most recently left first.
    pub fn former_chats(&mut self) -> Result<Vec<FormerChat>, CacheError> {
        lifecycle::former_chats(&mut self.conn)
    }

    /// Former chats left before `before` (ms), due for purging with [`Op::PurgeChannel`].
    pub fn expired_former_chats(&mut self, before: i64) -> Result<Vec<ChannelId>, CacheError> {
        lifecycle::expired_former_chats(&mut self.conn, before)
    }

    /// Moves the connection to the writer thread.
    pub fn into_writer(self) -> Result<Writer, CacheError> {
        Writer::start(self.conn)
    }
}

/// Searches through a reader connection ([`open_reader`]).
pub fn search(
    reader: &mut SqliteConnection,
    query: &str,
    channel: Option<&ChannelId>,
    limit: i64,
) -> Result<Vec<SearchHit>, CacheError> {
    search::search(reader, query, channel, limit)
}

/// Former chats through a reader connection.
pub fn former_chats(reader: &mut SqliteConnection) -> Result<Vec<FormerChat>, CacheError> {
    lifecycle::former_chats(reader)
}

/// A former chat's posts, newest first, through a reader connection.
pub fn former_posts(
    reader: &mut SqliteConnection,
    channel: &ChannelId,
    limit: i64,
) -> Result<Vec<mm_model::Post>, CacheError> {
    lifecycle::former_posts(reader, channel, limit)
}
