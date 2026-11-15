// SPDX-License-Identifier: AGPL-3.0-only

//! The accounts with local data (MatterLite.md §8.7): listed on the login screen with their
//! size and last use, each deletable on its own.

use std::path::{Path, PathBuf};

use diesel::prelude::*;
use mm_model::User;

use crate::error::CacheError;
use crate::ops::kv;
use crate::schema;

/// One account's local data: `state/servers/<account_id>/`.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredAccount {
    pub dir: PathBuf,
    pub server_url: Option<String>,
    pub user: User,
    /// Everything in the directory (cache, attachments later), in bytes.
    pub size: u64,
    /// When the account was last opened (ms), if known.
    pub last_used: Option<i64>,
    /// Posts not sent yet, listed before the data is deleted.
    pub unsent: usize,
}

/// Every account under `servers_dir` with a readable cache, most recently used first.
/// Directories that can't be read are skipped (logged).
pub fn stored_accounts(servers_dir: &Path) -> Vec<StoredAccount> {
    let Ok(entries) = std::fs::read_dir(servers_dir) else {
        return Vec::new();
    };
    let mut accounts: Vec<StoredAccount> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("cache.sqlite").is_file())
        .filter_map(|entry| match read_account(&entry.path()) {
            Ok(account) => account,
            Err(error) => {
                log::warn!("stored account {}: {error}", entry.path().display());
                None
            }
        })
        .collect();
    accounts.sort_by_key(|account| std::cmp::Reverse(account.last_used));
    accounts
}

fn read_account(dir: &Path) -> Result<Option<StoredAccount>, CacheError> {
    let mut conn = crate::db::open_reader(&dir.join("cache.sqlite"))?;
    let get = |conn: &mut diesel::sqlite::SqliteConnection, key: &str| {
        schema::kv::table
            .find(key)
            .select(schema::kv::value)
            .first::<String>(conn)
            .optional()
    };
    let Some(me) = get(&mut conn, kv::ME)? else {
        return Ok(None);
    };
    let user: User =
        serde_json::from_str(&me).map_err(|source| CacheError::Decode { what: "me", source })?;
    let server_url = get(&mut conn, kv::SERVER_URL)?;
    let last_used = get(&mut conn, kv::LAST_USED)?.and_then(|value| value.parse().ok());
    let unsent = schema::outbox::table.count().get_result::<i64>(&mut conn)? as usize;
    Ok(Some(StoredAccount {
        dir: dir.to_owned(),
        server_url,
        user,
        size: dir_size(dir),
        last_used,
        unsent,
    }))
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Deletes one account's local data. Its engine must not be running.
pub fn delete_account(dir: &Path) -> Result<(), CacheError> {
    std::fs::remove_dir_all(dir).map_err(|source| CacheError::Io {
        path: dir.to_owned(),
        source,
    })
}

/// Deletes the data of accounts not used for `days` (Settings → Storage, off by default).
/// Returns the deleted directories.
pub fn delete_unused_accounts(servers_dir: &Path, days: u32, now: i64) -> Vec<PathBuf> {
    let cutoff = now - i64::from(days) * 24 * 60 * 60 * 1000;
    stored_accounts(servers_dir)
        .into_iter()
        .filter(|account| account.last_used.is_some_and(|used| used < cutoff))
        .filter_map(|account| match delete_account(&account.dir) {
            Ok(()) => Some(account.dir),
            Err(error) => {
                log::warn!("deleting {}: {error}", account.dir.display());
                None
            }
        })
        .collect()
}

/// Prefix of the `kv` keys that hold UI state (open tabs, last team, …).
pub const UI_STATE_PREFIX: &str = "ui.";

/// The UI state saved in an account's cache, keys without [`UI_STATE_PREFIX`]. Empty when the
/// account has no cache yet or it can't be read (logged).
pub fn ui_state(account_dir: &Path) -> std::collections::HashMap<String, String> {
    let path = account_dir.join("cache.sqlite");
    if !path.is_file() {
        return Default::default();
    }
    let read = || -> Result<Vec<(String, String)>, CacheError> {
        let mut conn = crate::db::open_reader(&path)?;
        Ok(schema::kv::table
            .filter(schema::kv::key.like(format!("{UI_STATE_PREFIX}%")))
            .select((schema::kv::key, schema::kv::value))
            .load::<(String, String)>(&mut conn)?)
    };
    match read() {
        Ok(rows) => rows
            .into_iter()
            .map(|(key, value)| (key[UI_STATE_PREFIX.len()..].to_owned(), value))
            .collect(),
        Err(error) => {
            log::warn!("UI state in {}: {error}", path.display());
            Default::default()
        }
    }
}
