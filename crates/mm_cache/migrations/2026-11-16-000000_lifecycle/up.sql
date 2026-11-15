-- Lifecycle (MatterLite.md §8.7): unsent posts, and chats I'm no longer a member of.

-- Posts being sent, kept across restarts. The server de-duplicates by pending_post_id for
-- 30 s only, so older entries are reconciled before any resend.
CREATE TABLE outbox (
    pending_post_id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL,
    root_id TEXT NOT NULL,
    message TEXT NOT NULL,
    props_json TEXT NOT NULL,
    file_ids TEXT NOT NULL,            -- JSON array
    first_attempt_at BIGINT NOT NULL,
    last_attempt_at BIGINT NOT NULL,
    state TEXT NOT NULL                -- 'sending' or 'failed'
);

-- Channels whose membership went (left, removed, archived for me, team left). Their channel
-- row and posts stay, read-only, until the former-chat retention runs out; rejoining removes
-- the row here.
CREATE TABLE former_chats (
    channel_id TEXT PRIMARY KEY NOT NULL,
    left_at BIGINT NOT NULL
);
