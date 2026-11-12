// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warpui_extras/src/secure_storage/unavailable_tests.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

use super::SecureStorage;
use crate::secure_storage::{Error, SecureStorage as _};

#[test]
fn read_value_returns_not_found() {
    let storage = SecureStorage;

    assert!(matches!(storage.read_value("key"), Err(Error::NotFound)));
}

#[test]
fn write_value_is_discarded() {
    let storage = SecureStorage;

    storage.write_value("key", "value").expect("write succeeds");

    assert!(matches!(storage.read_value("key"), Err(Error::NotFound)));
}

#[test]
fn remove_value_succeeds() {
    let storage = SecureStorage;

    storage.remove_value("key").expect("remove succeeds");
}
