// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warpui_extras/src/secure_storage/noop.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! No-op [`SecureStorage`] service for use in unit and integration tests.

use super::Error;

#[derive(Default)]
pub struct SecureStorage {}

impl SecureStorage {
    pub fn new(_service_name: &str) -> Self {
        Self {}
    }
}

impl super::SecureStorage for SecureStorage {
    fn write_value(&self, _key: &str, _value: &str) -> Result<(), Error> {
        Ok(())
    }

    fn read_value(&self, _key: &str) -> Result<String, Error> {
        Ok("".to_string())
    }

    fn remove_value(&self, _key: &str) -> Result<(), Error> {
        Ok(())
    }
}
