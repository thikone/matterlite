// SPDX-License-Identifier: AGPL-3.0-only

//! Display names for users and channels (WebApp.md §14).

use mm_model::{User, UserId};

/// How other users' names are shown: the `display_settings`/`name_format` preference, else
/// the server's `TeammateNameDisplay` (`getTeammateNameDisplaySetting`). A licensed server
/// can lock it to the server setting (`LockTeammateNameDisplay`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameFormat {
    #[default]
    Username,
    /// Nickname, else full name, else username.
    NicknameFullName,
    /// Full name, else username.
    FullName,
}

impl NameFormat {
    pub fn parse(value: &str) -> Self {
        match value {
            "nickname_full_name" => Self::NicknameFullName,
            "full_name" => Self::FullName,
            _ => Self::Username,
        }
    }

    /// `displayUsername` in `MR/utils/user_utils.ts`.
    pub fn display(self, user: &User) -> String {
        let name = match self {
            Self::NicknameFullName if !user.nickname.trim().is_empty() => user.nickname.clone(),
            Self::NicknameFullName | Self::FullName => full_name(user),
            Self::Username => remote_username(user).unwrap_or(&user.username).to_owned(),
        };
        if name.trim().is_empty() {
            user.username.clone()
        } else {
            name
        }
    }
}

/// `first_name last_name`, or either one, or empty (`getFullName`).
pub fn full_name(user: &User) -> String {
    match (user.first_name.trim(), user.last_name.trim()) {
        ("", "") => String::new(),
        (first, "") => first.to_owned(),
        ("", last) => last.to_owned(),
        (first, last) => format!("{first} {last}"),
    }
}

/// A shared-channel user's name on their own server.
fn remote_username(user: &User) -> Option<&str> {
    user.remote_id.as_ref().filter(|id| !id.is_empty())?;
    user.props
        .get("RemoteUsername")
        .map(String::as_str)
        .filter(|name| !name.is_empty())
}

/// The other user of a DM channel, from its name `<id>__<id>` (sorted ids). A self-DM names
/// the user twice.
pub fn dm_partner(channel_name: &str, me: &UserId) -> Option<UserId> {
    let (first, second) = channel_name.split_once("__")?;
    Some(UserId::new(if first == me.as_str() {
        second
    } else {
        first
    }))
}

#[cfg(test)]
#[path = "names_tests.rs"]
mod tests;
