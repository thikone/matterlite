// SPDX-License-Identifier: AGPL-3.0-only

//! Profile pictures and custom emoji images (MatterLite.md §12): downloaded when the UI asks
//! for them and kept in the account's folder, so they show at once (and offline) later.
//!
//! ```text
//! <account dir>/images/avatars/<user id>-<last_picture_update>
//! <account dir>/images/emoji/<emoji id>
//! ```
//!
//! A new `last_picture_update` is a new file; the older pictures of that user are deleted when
//! it arrives. Custom emoji never change (a renamed or re-uploaded one gets a new id). The
//! files have no extension: the image loader reads the format from the content.

use std::path::{Path, PathBuf};

use mm_model::{EmojiId, UserId};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageKey {
    Avatar {
        user: UserId,
        /// The user's `last_picture_update`.
        updated: i64,
    },
    Emoji(EmojiId),
}

impl ImageKey {
    /// Where the image is (or will be) stored.
    pub fn path(&self, account_dir: &Path) -> PathBuf {
        match self {
            Self::Avatar { user, updated } => {
                avatars_dir(account_dir).join(format!("{user}-{updated}"))
            }
            Self::Emoji(id) => account_dir.join("images").join("emoji").join(id.as_str()),
        }
    }
}

fn avatars_dir(account_dir: &Path) -> PathBuf {
    account_dir.join("images").join("avatars")
}

/// Writes the image atomically (a temporary file renamed into place) and removes the user's
/// older pictures.
pub(crate) fn save(account_dir: &Path, key: &ImageKey, bytes: &[u8]) -> std::io::Result<()> {
    let path = key.path(account_dir);
    let dir = path.parent().expect("image paths have a parent");
    std::fs::create_dir_all(dir)?;
    let temporary = path.with_extension("part");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &path)?;
    if let ImageKey::Avatar { user, updated } = key {
        let prefix = format!("{user}-");
        let current = format!("{user}-{updated}");
        for entry in std::fs::read_dir(dir)?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name != current {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
