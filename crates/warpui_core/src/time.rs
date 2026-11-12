// SPDX-License-Identifier: MIT
// Derived from warp-terminal `crates/warpui_core/src/time.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

#[cfg(test)]
use std::sync::atomic::{AtomicI64, Ordering};

#[cfg(test)]
use chrono::TimeZone;
use chrono::{DateTime, Utc};

#[cfg(not(test))]
pub fn get_current_time() -> DateTime<Utc> {
    Utc::now()
}

thread_local! {
    #[cfg(test)]
    static CURRENT_SECS: AtomicI64 = const { AtomicI64::new(0) };
}
#[cfg(test)]
pub fn get_current_time() -> DateTime<Utc> {
    CURRENT_SECS.with(|current_secs| {
        Utc.timestamp_opt(current_secs.load(Ordering::SeqCst), 0)
            .unwrap()
    })
}
#[cfg(test)]
pub fn test_offset_time(offset_secs: i64) {
    CURRENT_SECS.with(|current_secs| {
        current_secs.fetch_add(offset_secs, Ordering::SeqCst);
    })
}
