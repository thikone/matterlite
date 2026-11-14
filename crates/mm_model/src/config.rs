// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::de::string_map;

/// The client config (`GET /config/client?format=old`) or client license
/// (`GET /license/client`): a flat map of strings, with booleans as `"true"`/`"false"`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientConfig(#[serde(deserialize_with = "string_map")] pub HashMap<String, String>);

impl ClientConfig {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// `true` only for the string `"true"`; a missing key is `false`.
    pub fn bool(&self, key: &str) -> bool {
        self.get(key) == Some("true")
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.parse().ok()
    }

    /// A value that is set and not empty.
    fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|value| !value.is_empty())
    }

    pub fn site_url(&self) -> Option<&str> {
        self.non_empty("SiteURL")
    }

    /// The WebSocket base URL when the admin set one (`ServiceSettings.WebsocketURL`).
    pub fn websocket_url(&self) -> Option<&str> {
        self.non_empty("WebsocketURL")
    }

    pub fn version(&self) -> Option<&str> {
        self.non_empty("Version")
    }

    pub fn collapsed_threads(&self) -> CollapsedThreads {
        CollapsedThreads::from_config(self.get("CollapsedThreads").unwrap_or_default())
    }
}

/// The server's Collapsed Reply Threads setting (`ServiceSettings.CollapsedThreads`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapsedThreads {
    Disabled,
    DefaultOff,
    DefaultOn,
    AlwaysOn,
}

impl CollapsedThreads {
    /// Unknown or missing values (servers before CRT) count as disabled.
    pub fn from_config(value: &str) -> Self {
        match value {
            "default_off" => Self::DefaultOff,
            "default_on" => Self::DefaultOn,
            "always_on" => Self::AlwaysOn,
            _ => Self::Disabled,
        }
    }

    /// Whether CRT applies for a user whose `display_settings`/`collapsed_reply_threads`
    /// preference is `preference` (`"on"`/`"off"`, `None` when not set). Mirrors the webapp's
    /// `isCollapsedThreadsEnabled`.
    pub fn is_enabled(self, preference: Option<&str>) -> bool {
        match self {
            Self::Disabled => false,
            Self::AlwaysOn => true,
            Self::DefaultOn => preference != Some("off"),
            Self::DefaultOff => preference == Some("on"),
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
