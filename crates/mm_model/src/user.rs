// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::de::string_map;
use crate::ids::UserId;

/// A user profile (`server/public/model/user.go`). Profiles of other users are sanitized:
/// `email` and some names may be empty depending on the server's privacy settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: UserId,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub username: String,
    pub auth_service: String,
    pub email: String,
    pub nickname: String,
    pub first_name: String,
    pub last_name: String,
    pub position: String,
    /// Space-separated role names (`"system_user system_admin"`).
    pub roles: String,
    #[serde(deserialize_with = "string_map")]
    pub props: HashMap<String, String>,
    /// `desktop`, `desktop_sound`, `mention_keys`, `channel`, `first_name`, ... (WebApp.md §13).
    #[serde(deserialize_with = "string_map")]
    pub notify_props: HashMap<String, String>,
    pub last_picture_update: i64,
    pub locale: String,
    /// `useAutomaticTimezone`, `automaticTimezone`, `manualTimezone`.
    #[serde(deserialize_with = "string_map")]
    pub timezone: HashMap<String, String>,
    pub mfa_active: bool,
    pub remote_id: Option<String>,
    pub last_activity_at: i64,
    pub is_bot: bool,
    pub bot_description: String,
}

impl User {
    pub fn is_deleted(&self) -> bool {
        self.delete_at != 0
    }

    pub fn role_names(&self) -> impl Iterator<Item = &str> {
        self.roles.split_whitespace()
    }
}

/// A user's presence (`server/public/model/status.go`). `status` is `online`, `away`, `dnd`,
/// `offline` or `ooo`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub user_id: UserId,
    pub status: String,
    pub manual: bool,
    pub last_activity_at: i64,
    pub dnd_end_time: i64,
}
