// SPDX-License-Identifier: AGPL-3.0-only

//! Diesel table definitions for `migrations/` (written by hand; `posts_fts` is used through
//! `sql_query`).

diesel::table! {
    kv (key) {
        key -> Text,
        value -> Text,
    }
}

diesel::table! {
    users (id) {
        id -> Text,
        update_at -> BigInt,
        json -> Text,
    }
}

diesel::table! {
    teams (id) {
        id -> Text,
        json -> Text,
    }
}

diesel::table! {
    team_members (team_id) {
        team_id -> Text,
        json -> Text,
    }
}

diesel::table! {
    channels (id) {
        id -> Text,
        team_id -> Text,
        json -> Text,
    }
}

diesel::table! {
    channel_members (channel_id) {
        channel_id -> Text,
        json -> Text,
    }
}

diesel::table! {
    posts (id) {
        id -> Text,
        channel_id -> Text,
        root_id -> Text,
        user_id -> Text,
        create_at -> BigInt,
        update_at -> BigInt,
        json -> Text,
    }
}

diesel::table! {
    timeline_ranges (owner_kind, owner_id, oldest_at, oldest_id) {
        owner_kind -> Text,
        owner_id -> Text,
        oldest_at -> BigInt,
        oldest_id -> Text,
        newest_at -> BigInt,
        newest_id -> Text,
        recent -> Bool,
        start -> Bool,
    }
}

diesel::table! {
    timeline_empty (owner_kind, owner_id) {
        owner_kind -> Text,
        owner_id -> Text,
    }
}

diesel::table! {
    preferences (category, name) {
        category -> Text,
        name -> Text,
        value -> Text,
    }
}

diesel::table! {
    categories (team_id) {
        team_id -> Text,
        json -> Text,
    }
}

diesel::table! {
    roles (name) {
        name -> Text,
        json -> Text,
    }
}

diesel::table! {
    emoji (name) {
        name -> Text,
        json -> Text,
    }
}

diesel::table! {
    threads (id) {
        id -> Text,
        json -> Text,
    }
}

diesel::table! {
    channel_sync (channel_id) {
        channel_id -> Text,
        newest_synced_at -> BigInt,
        backfilled_to -> Nullable<BigInt>,
        last_full_check -> BigInt,
        priority -> Integer,
    }
}
