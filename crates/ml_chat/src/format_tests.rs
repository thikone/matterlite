// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn sizes() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(999), "999 B");
    assert_eq!(format_size(1_000), "1.0 KB");
    assert_eq!(format_size(12_345), "12 KB");
    assert_eq!(format_size(3_400_000), "3.4 MB");
    assert_eq!(format_size(1_200_000_000), "1.2 GB");
}

#[test]
fn last_used() {
    let now = 100 * DAY_MS;
    assert_eq!(format_last_used(now - 20_000, now), "used just now");
    assert_eq!(format_last_used(now - 60_000, now), "used 1 minute ago");
    assert_eq!(
        format_last_used(now - 5 * 60_000, now),
        "used 5 minutes ago"
    );
    assert_eq!(
        format_last_used(now - 3 * 3_600_000, now),
        "used 3 hours ago"
    );
    assert_eq!(format_last_used(now - DAY_MS - 1, now), "used yesterday");
    assert_eq!(format_last_used(now - 12 * DAY_MS, now), "used 12 days ago");
    // A clock that went backwards.
    assert_eq!(format_last_used(now + 5_000, now), "used just now");
}
