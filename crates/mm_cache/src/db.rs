// SPDX-License-Identifier: AGPL-3.0-only

//! Opening the cache file, and applying [`Op`]s.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use mm_model::Post;

use crate::error::CacheError;
use crate::ops::Op;
use crate::schema;

pub(crate) const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

/// What happened when the cache was opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// The file couldn't be used (corrupt, or a migration failed) and was moved here; the
    /// cache starts empty and is refetched. The old file is kept so nothing is lost silently.
    pub moved_aside: Option<PathBuf>,
    pub reason: Option<String>,
}

/// Opens (creating if needed) the cache at `path`, migrated to the current schema. A broken
/// file is renamed to `<name>.broken-<unix seconds>` (with its `-wal`/`-shm`) and a new one
/// is created (MatterLite.md §8.3).
pub fn open(path: &Path) -> Result<(SqliteConnection, OpenReport), CacheError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| CacheError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    match open_and_migrate(path) {
        Ok(conn) => Ok((conn, OpenReport::default())),
        Err(error) if is_unusable(&error) && path.exists() => {
            let reason = error.to_string();
            log::warn!(
                "cache {} is unusable ({reason}); moving it aside",
                path.display()
            );
            let aside = move_aside(path)?;
            let conn = open_and_migrate(path)?;
            Ok((
                conn,
                OpenReport {
                    moved_aside: Some(aside),
                    reason: Some(reason),
                },
            ))
        }
        Err(error) => Err(error),
    }
}

/// A second connection for reads while the writer thread owns the first (WAL allows both).
pub fn open_reader(path: &Path) -> Result<SqliteConnection, CacheError> {
    let mut conn = connect(path)?;
    conn.batch_execute("PRAGMA query_only = ON;")?;
    Ok(conn)
}

fn connect(path: &Path) -> Result<SqliteConnection, CacheError> {
    let url = path
        .to_str()
        .ok_or_else(|| CacheError::Path(path.to_owned()))?;
    let mut conn = SqliteConnection::establish(url)?;
    conn.batch_execute(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 2000;
         PRAGMA wal_autocheckpoint = 1000;",
    )?;
    Ok(conn)
}

fn open_and_migrate(path: &Path) -> Result<SqliteConnection, CacheError> {
    let mut conn = connect(path)?;
    #[derive(QueryableByName)]
    struct Check {
        #[diesel(sql_type = Text)]
        quick_check: String,
    }
    let check: Vec<Check> = diesel::sql_query("PRAGMA quick_check").load(&mut conn)?;
    if check.first().map(|row| row.quick_check.as_str()) != Some("ok") {
        return Err(CacheError::Corrupt(
            check
                .into_iter()
                .map(|row| row.quick_check)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|error| CacheError::Migration(error.to_string()))?;
    Ok(conn)
}

/// Errors that mean the file itself is bad, as opposed to a locked or unreachable one.
fn is_unusable(error: &CacheError) -> bool {
    match error {
        CacheError::Corrupt(_) | CacheError::Migration(_) => true,
        CacheError::Query(diesel::result::Error::DatabaseError(_, info)) => {
            let message = info.message();
            message.contains("malformed") || message.contains("not a database")
        }
        CacheError::Connect(diesel::ConnectionError::BadConnection(message)) => {
            message.contains("not a database") || message.contains("malformed")
        }
        _ => false,
    }
}

fn move_aside(path: &Path) -> Result<PathBuf, CacheError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let aside = path.with_file_name(format!(
        "{}.broken-{seconds}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache.sqlite")
    ));
    let io = |source| CacheError::Io {
        path: path.to_owned(),
        source,
    };
    std::fs::rename(path, &aside).map_err(io)?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", path.display()));
        if side.exists() {
            let target = PathBuf::from(format!("{}{suffix}", aside.display()));
            std::fs::rename(&side, target).map_err(io)?;
        }
    }
    Ok(aside)
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("entities serialize")
}

/// Applies ops in one transaction.
pub(crate) fn apply(conn: &mut SqliteConnection, ops: &[Op]) -> Result<(), CacheError> {
    conn.immediate_transaction(|conn| {
        for op in ops {
            apply_one(conn, op)?;
        }
        Ok::<_, diesel::result::Error>(())
    })?;
    Ok(())
}

fn apply_one(conn: &mut SqliteConnection, op: &Op) -> QueryResult<()> {
    use schema::*;
    match op {
        Op::PutUser(user) => {
            diesel::replace_into(users::table)
                .values((
                    users::id.eq(user.id.as_str()),
                    users::update_at.eq(user.update_at),
                    users::json.eq(json(user)),
                ))
                .execute(conn)?;
        }
        Op::PutTeam(team) => {
            diesel::replace_into(teams::table)
                .values((teams::id.eq(team.id.as_str()), teams::json.eq(json(team))))
                .execute(conn)?;
        }
        Op::DeleteTeam(team) => {
            diesel::delete(teams::table.find(team.as_str())).execute(conn)?;
        }
        Op::PutTeamMember(member) => {
            diesel::replace_into(team_members::table)
                .values((
                    team_members::team_id.eq(member.team_id.as_str()),
                    team_members::json.eq(json(member)),
                ))
                .execute(conn)?;
        }
        Op::DeleteTeamMember(team) => {
            diesel::delete(team_members::table.find(team.as_str())).execute(conn)?;
        }
        Op::KeepTeams(keep) => {
            let keep: Vec<&str> = keep.iter().map(|id| id.as_str()).collect();
            diesel::delete(team_members::table.filter(team_members::team_id.ne_all(&keep)))
                .execute(conn)?;
        }
        Op::PutChannel(channel) => {
            diesel::replace_into(channels::table)
                .values((
                    channels::id.eq(channel.id.as_str()),
                    channels::team_id.eq(channel.team_id.as_str()),
                    channels::json.eq(json(channel)),
                ))
                .execute(conn)?;
        }
        Op::PutChannelMember(member) => {
            diesel::replace_into(channel_members::table)
                .values((
                    channel_members::channel_id.eq(member.channel_id.as_str()),
                    channel_members::json.eq(json(member)),
                ))
                .execute(conn)?;
        }
        Op::DeleteChannelMember(channel) => {
            diesel::delete(channel_members::table.find(channel.as_str())).execute(conn)?;
        }
        Op::KeepChannelMembers(keep) => {
            let keep: Vec<&str> = keep.iter().map(|id| id.as_str()).collect();
            diesel::delete(
                channel_members::table.filter(channel_members::channel_id.ne_all(&keep)),
            )
            .execute(conn)?;
        }
        Op::PutPost(post) => put_post(conn, post)?,
        Op::DeletePost(id) => {
            diesel::sql_query(
                "DELETE FROM posts_fts WHERE rowid = (SELECT rowid FROM posts WHERE id = ?)",
            )
            .bind::<Text, _>(id.as_str())
            .execute(conn)?;
            diesel::delete(posts::table.find(id.as_str())).execute(conn)?;
        }
        Op::SetRanges {
            owner,
            id,
            ranges,
            empty,
        } => {
            delete_ranges(conn, owner.as_str(), id)?;
            for range in ranges {
                diesel::insert_into(timeline_ranges::table)
                    .values((
                        timeline_ranges::owner_kind.eq(owner.as_str()),
                        timeline_ranges::owner_id.eq(id),
                        timeline_ranges::oldest_at.eq(range.oldest.0),
                        timeline_ranges::oldest_id.eq(range.oldest.1.as_str()),
                        timeline_ranges::newest_at.eq(range.newest.0),
                        timeline_ranges::newest_id.eq(range.newest.1.as_str()),
                        timeline_ranges::recent.eq(range.recent),
                        timeline_ranges::start.eq(range.start),
                    ))
                    .execute(conn)?;
            }
            if *empty {
                diesel::insert_into(timeline_empty::table)
                    .values((
                        timeline_empty::owner_kind.eq(owner.as_str()),
                        timeline_empty::owner_id.eq(id),
                    ))
                    .execute(conn)?;
            }
        }
        Op::DeleteRanges { owner, id } => delete_ranges(conn, owner.as_str(), id)?,
        Op::ReplacePreferences(all) => {
            diesel::delete(preferences::table).execute(conn)?;
            for (category, name, value) in all {
                diesel::insert_into(preferences::table)
                    .values((
                        preferences::category.eq(category),
                        preferences::name.eq(name),
                        preferences::value.eq(value),
                    ))
                    .execute(conn)?;
            }
        }
        Op::PutCategories(team, list) => {
            diesel::replace_into(categories::table)
                .values((
                    categories::team_id.eq(team.as_str()),
                    categories::json.eq(json(list)),
                ))
                .execute(conn)?;
        }
        Op::DeleteCategories(team) => {
            diesel::delete(categories::table.find(team.as_str())).execute(conn)?;
        }
        Op::ReplaceRoles(list) => {
            diesel::delete(roles::table).execute(conn)?;
            for role in list {
                diesel::insert_into(roles::table)
                    .values((roles::name.eq(&role.name), roles::json.eq(json(role))))
                    .execute(conn)?;
            }
        }
        Op::PutEmoji(item) => {
            diesel::replace_into(emoji::table)
                .values((emoji::name.eq(&item.name), emoji::json.eq(json(item))))
                .execute(conn)?;
        }
        Op::PutThread(thread) => {
            diesel::replace_into(threads::table)
                .values((
                    threads::id.eq(thread.id.as_str()),
                    threads::json.eq(json(thread)),
                ))
                .execute(conn)?;
        }
        Op::DeleteThread(root) => {
            diesel::delete(threads::table.find(root.as_str())).execute(conn)?;
        }
        Op::Kv(key, Some(value)) => {
            diesel::replace_into(kv::table)
                .values((kv::key.eq(key), kv::value.eq(value)))
                .execute(conn)?;
        }
        Op::Kv(key, None) => {
            diesel::delete(kv::table.find(key)).execute(conn)?;
        }
    }
    Ok(())
}

fn delete_ranges(conn: &mut SqliteConnection, owner: &str, id: &str) -> QueryResult<()> {
    use schema::{timeline_empty, timeline_ranges};
    diesel::delete(
        timeline_ranges::table
            .filter(timeline_ranges::owner_kind.eq(owner))
            .filter(timeline_ranges::owner_id.eq(id)),
    )
    .execute(conn)?;
    diesel::delete(
        timeline_empty::table
            .filter(timeline_empty::owner_kind.eq(owner))
            .filter(timeline_empty::owner_id.eq(id)),
    )
    .execute(conn)?;
    Ok(())
}

/// Upserts the post (keeping its rowid, which the search index uses) and re-indexes it.
fn put_post(conn: &mut SqliteConnection, post: &Post) -> QueryResult<()> {
    use schema::posts;
    if post.is_deleted() {
        return apply_one(conn, &Op::DeletePost(post.id.clone()));
    }
    diesel::insert_into(posts::table)
        .values((
            posts::id.eq(post.id.as_str()),
            posts::channel_id.eq(post.channel_id.as_str()),
            posts::root_id.eq(post.root_id.as_str()),
            posts::user_id.eq(post.user_id.as_str()),
            posts::create_at.eq(post.create_at),
            posts::update_at.eq(post.update_at),
            posts::json.eq(json(post)),
        ))
        .on_conflict(posts::id)
        .do_update()
        .set((
            posts::channel_id.eq(post.channel_id.as_str()),
            posts::root_id.eq(post.root_id.as_str()),
            posts::create_at.eq(post.create_at),
            posts::update_at.eq(post.update_at),
            posts::json.eq(json(post)),
        ))
        .execute(conn)?;
    #[derive(QueryableByName)]
    struct RowId {
        #[diesel(sql_type = BigInt)]
        rowid: i64,
    }
    let row: RowId = diesel::sql_query("SELECT rowid FROM posts WHERE id = ?")
        .bind::<Text, _>(post.id.as_str())
        .get_result(conn)?;
    diesel::sql_query("DELETE FROM posts_fts WHERE rowid = ?")
        .bind::<BigInt, _>(row.rowid)
        .execute(conn)?;
    let (text, file_names) = crate::search::index_text(post);
    if !text.is_empty() || !file_names.is_empty() {
        diesel::sql_query(
            "INSERT INTO posts_fts (rowid, channel_id, text, file_names) VALUES (?, ?, ?, ?)",
        )
        .bind::<BigInt, _>(row.rowid)
        .bind::<Text, _>(post.channel_id.as_str())
        .bind::<Text, _>(text)
        .bind::<Text, _>(file_names)
        .execute(conn)?;
    }
    Ok(())
}
