// SPDX-License-Identifier: AGPL-3.0-only

use super::ServerVersion;

#[test]
fn parses_release_and_dev_builds() {
    let release = ServerVersion::parse("10.11.0.123.abc.true").unwrap();
    assert_eq!((release.major, release.minor, release.patch), (10, 11, 0));
    assert_eq!(release.build, "123");
    assert_eq!(release.config_hash, "abc");
    assert!(release.licensed);
    assert_eq!(release.to_string(), "10.11.0 (build 123)");

    // From a live dev server: empty build number.
    let dev = ServerVersion::parse("12.2.0..a16ea064e55a.false").unwrap();
    assert_eq!(dev.build, "");
    assert_eq!(dev.config_hash, "a16ea064e55a");
    assert!(!dev.licensed);
    assert_eq!(dev.to_string(), "12.2.0");
    assert!(dev.at_least(12, 2, 0) && dev.at_least(9, 99, 99) && !dev.at_least(12, 3, 0));
}

#[test]
fn tolerates_dotted_builds_and_bare_versions() {
    let dotted = ServerVersion::parse("9.5.1.9.5.1.hash.false").unwrap();
    assert_eq!(dotted.build, "9.5.1");
    assert_eq!(dotted.config_hash, "hash");

    let bare = ServerVersion::parse("9.5.1").unwrap();
    assert_eq!((bare.build.as_str(), bare.config_hash.as_str()), ("", ""));
    assert!(ServerVersion::parse("dev").is_none());
}
