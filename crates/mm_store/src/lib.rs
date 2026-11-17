// SPDX-License-Identifier: AGPL-3.0-only

//! The normalized client state of one account on one server (MatterLite.md §8.1).
//!
//! Like the webapp's redux `entities` (WebApp.md §8.3): entities by id, my memberships with
//! their counters, timelines per channel and thread, preferences, categories and roles.
//! [`Store::apply`] takes WebSocket events, the loaders take REST responses, and every change
//! is reported as [`StoreDelta`]s for the UI copy and the cache writer. Derived views (unread
//! counts, team badges, display names, permissions, the sidebar and the timeline items) are
//! computed from the state. No I/O and
//! no UI: the sync engine (`mm_sync`) drives it.

mod actions;
mod apply;
mod delta;
mod items;
mod names;
mod sidebar;
mod store;
mod timeline;
mod unread;

pub use actions::PostActions;
pub use delta::{Fetch, StoreDelta};
pub use items::{
    ItemOptions, MAX_COMBINED_SYSTEM_POSTS, POST_COLLAPSE_MS, TimelineItem,
    USER_ACTIVITY_POST_TYPES, Zone, consecutive,
};
pub use names::{NameFormat, dm_partner, full_name};
pub use sidebar::{
    DEFAULT_DM_GM_LIMIT, Sidebar, SidebarCategoryView, SidebarChannel, compare_names,
};
pub use store::{PageRequest, PendingPost, Store, ThreadCounts};
pub use timeline::{Key, Page, Range, Timeline};
pub use unread::{
    ChannelUnread, JOIN_LEAVE_POST_TYPES, TeamUnread, counts_as_message,
    is_notification_suppressed, is_own_message, is_urgent,
};
