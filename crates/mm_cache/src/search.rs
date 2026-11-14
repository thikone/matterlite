// SPDX-License-Identifier: AGPL-3.0-only

//! Local message search over the cache (History pane, MatterLite.md §10.8).

use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::sqlite::SqliteConnection;
use mm_model::{ChannelId, Post, PostId};

use crate::error::CacheError;

/// One match, best first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub post: PostId,
    pub channel: ChannelId,
    pub create_at: i64,
}

/// What a post contributes to the index: its message plus the text of message attachments
/// (bot and webhook posts), and its file names.
pub(crate) fn index_text(post: &Post) -> (String, String) {
    let mut text = post.message.clone();
    if let Some(attachments) = post
        .props
        .get("attachments")
        .and_then(|value| value.as_array())
    {
        for attachment in attachments {
            for key in ["pretext", "title", "text", "fallback"] {
                if let Some(part) = attachment.get(key).and_then(|value| value.as_str())
                    && !part.is_empty()
                {
                    text.push('\n');
                    text.push_str(part);
                }
            }
            for field in attachment
                .get("fields")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
            {
                for key in ["title", "value"] {
                    if let Some(part) = field.get(key).and_then(|value| value.as_str()) {
                        text.push('\n');
                        text.push_str(part);
                    }
                }
            }
        }
    }
    let file_names = post
        .metadata
        .as_ref()
        .map(|metadata| {
            metadata
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    (text, file_names)
}

/// Turns what the user typed into an FTS5 query: every word must match, the last one as a
/// prefix (search as you type). Quotes keep FTS5 operators in the input literal.
pub fn fts_query(input: &str) -> Option<String> {
    let words: Vec<String> = input
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    let (last, rest) = words.split_last()?;
    let mut query = rest.join(" ");
    if !query.is_empty() {
        query.push(' ');
    }
    query.push_str(last);
    query.push('*');
    Some(query)
}

/// Searches messages, attachment text and file names; best match first, then newest.
pub fn search(
    conn: &mut SqliteConnection,
    input: &str,
    channel: Option<&ChannelId>,
    limit: i64,
) -> Result<Vec<SearchHit>, CacheError> {
    let Some(query) = fts_query(input) else {
        return Ok(Vec::new());
    };
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Text)]
        channel_id: String,
        #[diesel(sql_type = BigInt)]
        create_at: i64,
    }
    let rows: Vec<Row> = diesel::sql_query(
        "SELECT p.id, p.channel_id, p.create_at
         FROM posts_fts f JOIN posts p ON p.rowid = f.rowid
         WHERE posts_fts MATCH ? AND (? IS NULL OR f.channel_id = ?)
         ORDER BY bm25(posts_fts), p.create_at DESC
         LIMIT ?",
    )
    .bind::<Text, _>(&query)
    .bind::<Nullable<Text>, _>(channel.map(ChannelId::as_str))
    .bind::<Nullable<Text>, _>(channel.map(ChannelId::as_str))
    .bind::<BigInt, _>(limit)
    .load(conn)?;
    Ok(rows
        .into_iter()
        .map(|row| SearchHit {
            post: PostId::new(row.id),
            channel: ChannelId::new(row.channel_id),
            create_at: row.create_at,
        })
        .collect())
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
