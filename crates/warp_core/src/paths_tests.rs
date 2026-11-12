// SPDX-License-Identifier: AGPL-3.0-only
// Rewritten for MatterLite; Warp's `crates/warp_core/src/paths.rs` @ 5bb7bcc6a (portable mode) was the reference.

use std::path::Path;

use super::*;

#[test]
fn portable_layout_is_under_the_data_directory() {
    let roots = Roots::portable(Path::new("/opt/matterlite/data"));
    assert_eq!(roots.config, Path::new("/opt/matterlite/data/config"));
    assert_eq!(roots.state, Path::new("/opt/matterlite/data/state"));
    assert_eq!(roots.cache, Path::new("/opt/matterlite/data/cache"));
}

#[test]
fn installed_layout_keeps_config_state_and_cache_apart() {
    let roots = Roots::installed();
    assert_ne!(roots.config, roots.state);
    assert_ne!(roots.state, roots.cache);
    assert!(roots.config.ends_with(APP_DIR_NAME) || roots.config.ends_with(APP_DIR_NAME_LOWERCASE));
}
