// SPDX-License-Identifier: AGPL-3.0-only

//! Unsent posts and former chats (MatterLite.md §8.7).

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::SqliteConnection;
use mm_model::{Channel, ChannelId, FileId, Post, PostId};

use crate::error::CacheError;
use crate::schema;

/// A post being sent, kept across restarts.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboxEntry {
    pub pending_post_id: String,
    pub channel_id: ChannelId,
    pub root_id: PostId,
    pub message: String,
    pub props: serde_json::Map<String, serde_json::Value>,
    pub file_ids: Vec<FileId>,
    pub first_attempt_at: i64,
    pub last_attempt_at: i64,
    /// Sending failed (or couldn't be confirmed after a restart): shown with Retry/Delete.
    pub failed: bool,
}

/// A chat I'm no longer a member of; its posts stay until the retention runs out.
#[derive(Clone, Debug, PartialEq)]
pub struct FormerChat {
    pub channel: Channel,
    pub left_at: i64,
    pub posts: i64,
}

/// An `outbox` row: pending id, channel, root, message, props, file ids, first and last
/// attempt, state.
type OutboxRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
);

pub(crate) fn outbox(conn: &mut SqliteConnection) -> Result<Vec<OutboxEntry>, CacheError> {
    use schema::outbox::dsl::*;
    let rows: Vec<OutboxRow> = outbox
        .select((
            pending_post_id,
            channel_id,
            root_id,
            message,
            props_json,
            file_ids,
            first_attempt_at,
            last_attempt_at,
            state,
        ))
        .order(first_attempt_at.asc())
        .load(conn)?;
    rows.into_iter()
        .map(
            |(pending, channel, root, text, props, files, first, last, row_state)| {
                Ok(OutboxEntry {
                    pending_post_id: pending,
                    channel_id: ChannelId::new(channel),
                    root_id: PostId::new(root),
                    message: text,
                    props: serde_json::from_str(&props).map_err(|source| CacheError::Decode {
                        what: "outbox props",
                        source,
                    })?,
                    file_ids: serde_json::from_str(&files).map_err(|source| {
                        CacheError::Decode {
                            what: "outbox file ids",
                            source,
                        }
                    })?,
                    first_attempt_at: first,
                    last_attempt_at: last,
                    failed: row_state == "failed",
                })
            },
        )
        .collect()
}

pub(crate) fn put_outbox(conn: &mut SqliteConnection, entry: &OutboxEntry) -> QueryResult<()> {
    use schema::outbox::dsl::*;
    diesel::replace_into(outbox)
        .values((
            pending_post_id.eq(&entry.pending_post_id),
            channel_id.eq(entry.channel_id.as_str()),
            root_id.eq(entry.root_id.as_str()),
            message.eq(&entry.message),
            props_json.eq(serde_json::Value::Object(entry.props.clone()).to_string()),
            file_ids.eq(serde_json::to_string(&entry.file_ids).expect("ids serialize")),
            first_attempt_at.eq(entry.first_attempt_at),
            last_attempt_at.eq(entry.last_attempt_at),
            state.eq(if entry.failed { "failed" } else { "sending" }),
        ))
        .execute(conn)?;
    Ok(())
}

pub(crate) fn former_chats(conn: &mut SqliteConnection) -> Result<Vec<FormerChat>, CacheError> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        json: String,
        #[diesel(sql_type = BigInt)]
        left_at: i64,
        #[diesel(sql_type = BigInt)]
        posts: i64,
    }
    let rows: Vec<Row> = diesel::sql_query(
        "SELECT c.json, f.left_at,
                (SELECT COUNT(*) FROM posts p WHERE p.channel_id = f.channel_id) AS posts
         FROM former_chats f JOIN channels c ON c.id = f.channel_id
         ORDER BY f.left_at DESC",
    )
    .load(conn)?;
    rows.into_iter()
        .map(|row| {
            Ok(FormerChat {
                channel: serde_json::from_str(&row.json).map_err(|source| CacheError::Decode {
                    what: "channel",
                    source,
                })?,
                left_at: row.left_at,
                posts: row.posts,
            })
        })
        .collect()
}

/// A former chat's posts, newest first (read-only history).
pub(crate) fn former_posts(
    conn: &mut SqliteConnection,
    channel: &ChannelId,
    limit: i64,
) -> Result<Vec<Post>, CacheError> {
    use schema::posts;
    posts::table
        .filter(posts::channel_id.eq(channel.as_str()))
        .order(posts::create_at.desc())
        .limit(limit)
        .select(posts::json)
        .load::<String>(conn)?
        .iter()
        .map(|json| {
            serde_json::from_str(json).map_err(|source| CacheError::Decode {
                what: "post",
                source,
            })
        })
        .collect()
}

/// Former chats left before `before` (ms): due for purging.
pub(crate) fn expired_former_chats(
    conn: &mut SqliteConnection,
    before: i64,
) -> Result<Vec<ChannelId>, CacheError> {
    use schema::former_chats;
    Ok(former_chats::table
        .filter(former_chats::left_at.lt(before))
        .select(former_chats::channel_id)
        .load::<String>(conn)?
        .into_iter()
        .map(ChannelId::new)
        .collect())
}

/// Marks channels as former chats (keeping an earlier time if already marked).
pub(crate) fn mark_former(
    conn: &mut SqliteConnection,
    channels: &[String],
    at: i64,
) -> QueryResult<()> {
    for channel in channels {
        diesel::sql_query("INSERT OR IGNORE INTO former_chats (channel_id, left_at) VALUES (?, ?)")
            .bind::<Text, _>(channel)
            .bind::<BigInt, _>(at)
            .execute(conn)?;
    }
    Ok(())
}

/// Removes everything of a channel: posts with their index rows, timelines, threads rooted
/// there, membership, the channel and its former-chat mark.
pub(crate) fn purge_channel(conn: &mut SqliteConnection, channel: &ChannelId) -> QueryResult<()> {
    let id = channel.as_str();
    for statement in [
        "DELETE FROM posts_fts WHERE rowid IN (SELECT rowid FROM posts WHERE channel_id = ?)",
        "DELETE FROM timeline_ranges WHERE owner_kind = 'thread' AND owner_id IN
             (SELECT id FROM posts WHERE channel_id = ? AND root_id = '')",
        "DELETE FROM threads WHERE id IN (SELECT id FROM posts WHERE channel_id = ? AND root_id = '')",
        "DELETE FROM posts WHERE channel_id = ?",
        "DELETE FROM timeline_ranges WHERE owner_kind = 'channel' AND owner_id = ?",
        "DELETE FROM timeline_empty WHERE owner_kind = 'channel' AND owner_id = ?",
        "DELETE FROM channel_members WHERE channel_id = ?",
        "DELETE FROM channel_sync WHERE channel_id = ?",
        "DELETE FROM drafts WHERE channel_id = ?",
        "DELETE FROM former_chats WHERE channel_id = ?",
        "DELETE FROM channels WHERE id = ?",
    ] {
        diesel::sql_query(statement)
            .bind::<Text, _>(id)
            .execute(conn)?;
    }
    Ok(())
}
