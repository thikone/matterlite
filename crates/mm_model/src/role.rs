// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};

use crate::de::null_default;

/// A role with its permissions (`server/public/model/role.go`), from `POST /roles/names`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    #[serde(deserialize_with = "null_default")]
    pub permissions: Vec<String>,
    pub scheme_managed: bool,
    pub built_in: bool,
}

impl Role {
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}
