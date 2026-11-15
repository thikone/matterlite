// SPDX-License-Identifier: AGPL-3.0-only

//! Chat settings in `settings.toml` (`[chats]`).

use settings::SupportedPlatforms;
use settings::macros::define_settings_group;

use crate::tabs::DEFAULT_MAX_TABS;

define_settings_group!(ChatSettings, settings: [
    max_open_tabs: MaxOpenTabs {
        type: usize,
        default: DEFAULT_MAX_TABS,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        toml_path: "chats.max_open_tabs",
        description: "How many unpinned chat tabs stay open; opening another closes the least recently used one.",
    },
]);
