// SPDX-License-Identifier: AGPL-3.0-only

use serde::{Deserialize, Serialize};

use crate::ids::UserId;

/// One user preference (`server/public/model/preference.go`). Useful categories and names are
/// listed in WebApp.md §8.2.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Preference {
    pub user_id: UserId,
    pub category: String,
    pub name: String,
    pub value: String,
}

/// Categories and names the client reads.
pub mod keys {
    pub const DISPLAY_SETTINGS: &str = "display_settings";
    pub const COLLAPSED_REPLY_THREADS: &str = "collapsed_reply_threads";
    pub const NAME_FORMAT: &str = "name_format";
    pub const DIRECT_CHANNEL_SHOW: &str = "direct_channel_show";
    pub const GROUP_CHANNEL_SHOW: &str = "group_channel_show";
    pub const THEME: &str = "theme";
}
