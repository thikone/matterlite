// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::UserId;
use sha2::{Digest, Sha256};

/// The directory name of one account's local data, `state/servers/<id>/` (MatterLite.md §8.3,
/// §8.7): a hash of the normalized server URL and the user id, so each account on each server
/// has its own cache and another user never sees it. 16 hex digits.
pub fn account_id(server_url: &str, user: &UserId) -> String {
    let digest = Sha256::new()
        .chain_update(server_url.trim_end_matches('/').as_bytes())
        .chain_update(b"\n")
        .chain_update(user.as_str().as_bytes())
        .finalize();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
