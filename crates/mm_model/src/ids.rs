// SPDX-License-Identifier: AGPL-3.0-only

//! Typed entity ids. Mattermost ids are 26 lowercase base32 characters; an empty id means
//! "none" on the wire (for example `root_id` of a root post).

use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(id)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

id_type!(
    /// A user (or bot) id.
    UserId
);
id_type!(
    /// A team id.
    TeamId
);
id_type!(
    /// A channel id (public, private, DM or GM).
    ChannelId
);
id_type!(
    /// A post id.
    PostId
);
id_type!(
    /// An uploaded file id.
    FileId
);
id_type!(
    /// A sidebar category id. Unlike the others, these are not plain 26-character ids
    /// (e.g. `favorites_<user>_<team>`).
    CategoryId
);
id_type!(
    /// A custom emoji id.
    EmojiId
);

/// Whether `id` has the shape of a Mattermost id (`[a-z0-9]{26}`).
pub fn is_valid_id(id: &str) -> bool {
    id.len() == 26
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}
