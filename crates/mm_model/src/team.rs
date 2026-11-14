// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};

use crate::ids::{TeamId, UserId};

/// A team (`server/public/model/team.go`). `team_type` is `O` (open) or `I` (invite only).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Team {
    pub id: TeamId,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub display_name: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub team_type: String,
    pub allow_open_invite: bool,
    pub last_team_icon_update: i64,
    pub scheme_id: Option<String>,
    pub group_constrained: Option<bool>,
}

/// The current user's membership in a team (`server/public/model/team_member.go`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeamMember {
    pub team_id: TeamId,
    pub user_id: UserId,
    pub roles: String,
    pub delete_at: i64,
    pub scheme_guest: bool,
    pub scheme_user: bool,
    pub scheme_admin: bool,
    pub explicit_roles: String,
}

/// Unread counts of one team (`GET /users/me/teams/unread`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeamUnread {
    pub team_id: TeamId,
    pub msg_count: i64,
    pub mention_count: i64,
    pub mention_count_root: i64,
    pub msg_count_root: i64,
    pub thread_count: i64,
    pub thread_mention_count: i64,
    pub thread_urgent_mention_count: i64,
}
