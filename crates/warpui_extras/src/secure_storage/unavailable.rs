// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/warpui_extras/src/secure_storage/unavailable.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! [`SecureStorage`] provider for processes that must not access persistent secrets.

use super::Error;

pub struct SecureStorage;

impl super::SecureStorage for SecureStorage {
    fn write_value(&self, _key: &str, _value: &str) -> Result<(), Error> {
        Ok(())
    }

    fn read_value(&self, _key: &str) -> Result<String, Error> {
        Err(Error::NotFound)
    }

    fn remove_value(&self, _key: &str) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "unavailable_tests.rs"]
mod tests;
