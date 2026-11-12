// SPDX-License-Identifier: AGPL-3.0-only
// Rewritten for MatterLite; Warp's `crates/warp_core/src/paths.rs` @ 5bb7bcc6a (portable mode) was the reference.

//! Where MatterLite keeps its files (MatterLite.md §9).
//!
//! **Portable mode**: when a directory named `data` exists next to the executable, everything
//! lives under it (`data/config`, `data/state`, `data/cache`). Delete the directory to go back
//! to the per-user locations; nothing is migrated in either direction.
//!
//! **Installed mode**: the per-user OS locations.
//!
//! | | config | state | cache |
//! |---|---|---|---|
//! | Windows | `%APPDATA%\MatterLite` | `%LOCALAPPDATA%\MatterLite\state` | `%LOCALAPPDATA%\MatterLite\cache` |
//! | macOS | `~/Library/Application Support/MatterLite` | `…/MatterLite/state` | `~/Library/Caches/MatterLite` |
//! | Linux | `$XDG_CONFIG_HOME/matterlite` | `$XDG_STATE_HOME/matterlite` | `$XDG_CACHE_HOME/matterlite` |
//!
//! `config` holds user-editable files (settings, keybindings, themes); `state` holds data the
//! app owns (per-server SQLite caches and attachments, logs, encrypted secrets); `cache` holds
//! re-fetchable data (avatars, custom emoji, link-preview images).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::BaseDirs;

/// Directory name that switches the app into portable mode when present next to the executable.
const PORTABLE_DATA_DIR_NAME: &str = "data";

/// Application directory name on Windows and macOS.
const APP_DIR_NAME: &str = "MatterLite";

/// Application directory name on Linux, following XDG conventions.
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
const APP_DIR_NAME_LOWERCASE: &str = "matterlite";

/// The three roots everything else is derived from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl Roots {
    /// The portable-mode layout under `root` (the `data` directory).
    pub fn portable(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
        }
    }

    /// The per-user OS locations. Falls back to a relative `MatterLite` directory if the home
    /// directory can't be determined.
    pub fn installed() -> Self {
        let Some(base) = BaseDirs::new() else {
            log::error!("Couldn't determine the home directory; using ./{APP_DIR_NAME}");
            return Self::portable(Path::new(APP_DIR_NAME));
        };
        cfg_if::cfg_if! {
            if #[cfg(target_os = "windows")] {
                let local = base.data_local_dir().join(APP_DIR_NAME);
                Self {
                    config: base.config_dir().join(APP_DIR_NAME),
                    state: local.join("state"),
                    cache: local.join("cache"),
                }
            } else if #[cfg(target_os = "macos")] {
                let support = base.config_dir().join(APP_DIR_NAME);
                Self {
                    state: support.join("state"),
                    config: support,
                    cache: base.cache_dir().join(APP_DIR_NAME),
                }
            } else {
                let state = base
                    .state_dir()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| base.home_dir().join(".local/state"));
                Self {
                    config: base.config_dir().join(APP_DIR_NAME_LOWERCASE),
                    state: state.join(APP_DIR_NAME_LOWERCASE),
                    cache: base.cache_dir().join(APP_DIR_NAME_LOWERCASE),
                }
            }
        }
    }
}

/// Returns the portable-mode root (`<exe dir>/data`), if this install is portable.
///
/// Resolved once per process from the executable's location, so the working directory the app
/// is launched from doesn't matter.
pub fn portable_root() -> Option<&'static Path> {
    static PORTABLE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    PORTABLE_ROOT
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let candidate = exe.parent()?.join(PORTABLE_DATA_DIR_NAME);
            candidate.is_dir().then_some(candidate)
        })
        .as_deref()
}

/// The roots for this process: portable if a `data` directory exists next to the executable,
/// installed otherwise.
pub fn roots() -> &'static Roots {
    static ROOTS: OnceLock<Roots> = OnceLock::new();
    ROOTS.get_or_init(|| match portable_root() {
        Some(root) => Roots::portable(root),
        None => Roots::installed(),
    })
}

/// User-editable configuration: settings, keybindings, custom themes.
pub fn config_dir() -> PathBuf {
    roots().config.clone()
}

/// Data the app owns and can't re-download cheaply: caches per server, logs, secrets.
pub fn state_dir() -> PathBuf {
    roots().state.clone()
}

/// Re-fetchable data that may be deleted at any time.
pub fn cache_dir() -> PathBuf {
    roots().cache.clone()
}

/// `settings.toml`: public, user-visible settings.
pub fn settings_file_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Private settings that are not meant for hand-editing (window state, last opened chats).
pub fn private_preferences_file_path() -> PathBuf {
    state_dir().join("user_preferences.json")
}

/// `keybindings.yaml`: user keybinding overrides.
pub fn keybindings_file_path() -> PathBuf {
    config_dir().join("keybindings.yaml")
}

/// Custom theme files.
pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

/// Per-server data: `<state>/servers/<server_id>/cache.sqlite` and `files/`.
pub fn servers_dir() -> PathBuf {
    state_dir().join("servers")
}

/// Log files.
pub fn logs_dir() -> PathBuf {
    state_dir().join("logs")
}

/// Directory for secrets that the platform can't keep in a system keystore (DPAPI-encrypted
/// token files on Windows).
pub fn secure_state_dir() -> PathBuf {
    state_dir().join("secure")
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
