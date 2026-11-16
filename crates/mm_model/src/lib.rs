// SPDX-License-Identifier: AGPL-3.0-only

//! Mattermost API types.
//!
//! Field names follow the Go structs in the server's `server/public/model` (the wire format).
//! Every struct is tolerant: missing fields take their defaults, unknown fields are ignored and
//! `null` for a list or map (a nil Go slice or map) reads as empty, because servers add fields
//! with every release. See `docs/WebApp.md` §8 for what the fields mean.

mod de;

pub mod channel;
pub mod config;
pub mod error;
pub mod ids;
pub mod post;
pub mod preference;
pub mod role;
pub mod team;
pub mod thread;
pub mod token;
pub mod user;
pub mod version;
pub mod ws;

pub use channel::{
    Channel, ChannelMember, ChannelStats, ChannelType, OrderedSidebarCategories, SidebarCategory,
};
pub use config::{ClientConfig, CollapsedThreads};
pub use error::AppError;
pub use ids::{CategoryId, ChannelId, EmojiId, FileId, PostId, TeamId, UserId, is_valid_id};
pub use post::{Emoji, FileInfo, Post, PostList, PostMetadata, Reaction};
pub use preference::Preference;
pub use role::Role;
pub use team::{Team, TeamMember, TeamUnread};
pub use thread::{ThreadList, ThreadResponse};
pub use token::SessionToken;
pub use user::{Status, User};
pub use version::ServerVersion;
