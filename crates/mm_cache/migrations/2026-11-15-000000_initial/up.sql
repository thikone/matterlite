-- MatterLite cache schema v1 (MatterLite.md §8.3). Server entities are stored as JSON blobs
-- (the wire format of mm_model) keyed by id, plus the few columns queries need, so server
-- schema changes don't need migrations here.

-- Account identity, the WebSocket resume state, config, license, thread counts, ...
CREATE TABLE kv (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    update_at BIGINT NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE teams (
    id TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE team_members (
    team_id TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

-- Channels stay when the membership goes ("former chats", §8.7); hydration shows only those
-- with a membership.
CREATE TABLE channels (
    id TEXT PRIMARY KEY NOT NULL,
    team_id TEXT NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE channel_members (
    channel_id TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE posts (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL,
    root_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    create_at BIGINT NOT NULL,
    update_at BIGINT NOT NULL,
    json TEXT NOT NULL
);
CREATE INDEX posts_channel_time ON posts (channel_id, create_at DESC);
CREATE INDEX posts_root ON posts (root_id) WHERE root_id <> '';

-- Contiguous loaded stretches of a channel timeline or a thread (mm_store::Range).
CREATE TABLE timeline_ranges (
    owner_kind TEXT NOT NULL,          -- 'channel' or 'thread'
    owner_id TEXT NOT NULL,
    oldest_at BIGINT NOT NULL,
    oldest_id TEXT NOT NULL,
    newest_at BIGINT NOT NULL,
    newest_id TEXT NOT NULL,
    recent BOOLEAN NOT NULL,
    start BOOLEAN NOT NULL,
    PRIMARY KEY (owner_kind, owner_id, oldest_at, oldest_id)
);

-- Timelines a page reported empty.
CREATE TABLE timeline_empty (
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    PRIMARY KEY (owner_kind, owner_id)
);

CREATE TABLE preferences (
    category TEXT NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (category, name)
);

-- A team's sidebar categories in display order, as one JSON array.
CREATE TABLE categories (
    team_id TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE roles (
    name TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE emoji (
    name TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

-- Followed threads (CRT), by root post id.
CREATE TABLE threads (
    id TEXT PRIMARY KEY NOT NULL,
    json TEXT NOT NULL
);

CREATE TABLE drafts (
    channel_id TEXT NOT NULL,
    root_id TEXT NOT NULL,
    message TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (channel_id, root_id)
);

-- Per-channel sync bookkeeping for the eager scheduler (§8.5).
CREATE TABLE channel_sync (
    channel_id TEXT PRIMARY KEY NOT NULL,
    newest_synced_at BIGINT NOT NULL DEFAULT 0,
    backfilled_to BIGINT,              -- oldest create_at fetched; 0 = reached the start
    last_full_check BIGINT NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 0
);

-- Local message search (History pane, §10.8). rowid = posts.rowid.
CREATE VIRTUAL TABLE posts_fts USING fts5 (
    channel_id UNINDEXED,
    text,
    file_names,
    tokenize = 'unicode61 remove_diacritics 2'
);
