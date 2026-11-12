// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;
use std::io::Write;

use super::*;

fn read(path: PathBuf) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn previous_launches_shift_up_and_the_oldest_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = |suffix: &str| dir.path().join(format!("app.log{suffix}"));
    fs::write(path(""), "current").unwrap();
    fs::write(path(".old.0"), "previous").unwrap();
    fs::write(path(".old.1"), "oldest").unwrap();

    rotate_previous_launches(dir.path(), "app.log", 2).unwrap();

    assert!(!path("").exists());
    assert_eq!(read(path(".old.0")), "current");
    assert_eq!(read(path(".old.1")), "previous");
    assert!(!path(".old.2").exists());
}

#[test]
fn first_launch_has_nothing_to_rotate() {
    let dir = tempfile::tempdir().unwrap();
    rotate_previous_launches(dir.path(), "app.log", 3).unwrap();
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn active_file_rotates_once_full() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = RotatingFileWriter::open(dir.path(), "app.log", 10, 2).unwrap();
    writer.write_all(b"12345678").unwrap();
    writer.write_all(b"abcdef").unwrap();
    writer.write_all(b"ghijklmn").unwrap();
    writer.flush().unwrap();

    assert_eq!(read(dir.path().join("app.log")), "ghijklmn");
    assert_eq!(read(dir.path().join("app.log.in_session.0")), "abcdef");
    assert_eq!(read(dir.path().join("app.log.in_session.1")), "12345678");
}

#[test]
fn oversized_first_write_stays_in_the_active_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = RotatingFileWriter::open(dir.path(), "app.log", 4, 1).unwrap();
    writer.write_all(b"too long for the limit").unwrap();
    writer.flush().unwrap();

    assert_eq!(read(dir.path().join("app.log")), "too long for the limit");
    assert!(!dir.path().join("app.log.in_session.0").exists());
}
