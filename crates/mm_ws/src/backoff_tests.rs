// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn matches_the_webapp() {
    let backoff = Backoff::default();
    let secs = |fails| backoff.delay(fails, 0.0).as_secs();
    assert_eq!(secs(1), 3);
    assert_eq!(secs(7), 3);
    assert_eq!(secs(8), 3 * 64);
    assert_eq!(secs(9), 3 * 81);
    assert_eq!(secs(10), 5 * 60, "3 s × 100 is capped at 5 min");
    assert_eq!(secs(u32::MAX), 5 * 60);
}

#[test]
fn jitter_is_added_on_top() {
    let backoff = Backoff::default();
    assert_eq!(backoff.delay(1, 0.5), Duration::from_secs(4));
    assert_eq!(backoff.delay(10, 0.999_999).as_secs(), 5 * 60 + 1);
}
