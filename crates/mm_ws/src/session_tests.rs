// SPDX-License-Identifier: AGPL-3.0-only

use serde_json::json;

use super::*;

fn frame(event: &str, seq: i64) -> EventFrame {
    EventFrame {
        event: event.to_owned(),
        data: json!({}),
        seq,
        ..EventFrame::default()
    }
}

fn hello(connection_id: &str) -> EventFrame {
    EventFrame {
        data: json!({ "connection_id": connection_id }),
        ..frame("hello", 0)
    }
}

#[test]
fn fresh_connection_then_gap() {
    let mut session = Session::default();
    assert!(!session.is_resuming());
    assert_eq!(
        session.check(&hello("c1")),
        Check::Accepted {
            missed_events: false
        }
    );
    assert_eq!(
        session.check(&frame("posted", 1)),
        Check::Accepted {
            missed_events: false
        }
    );
    assert_eq!(
        session.check(&frame("posted", 3)),
        Check::Gap {
            expected: 2,
            got: 3
        }
    );
    let base = Url::parse("ws://localhost/api/v4/websocket").unwrap();
    assert_eq!(
        session.connect_url(&base, Some(4001)).as_str(),
        "ws://localhost/api/v4/websocket?connection_id=c1&sequence_number=2&disconnect_err_code=4001"
    );
}

#[test]
fn a_new_connection_id_means_missed_events() {
    let mut session = Session::new(Some(ResumeState {
        connection_id: "c1".to_owned(),
        sequence_number: 40,
    }));
    assert!(session.is_resuming());
    assert_eq!(
        session.check(&hello("c2")),
        Check::Accepted {
            missed_events: true
        }
    );
    assert_eq!(
        session.check(&frame("typing", 1)),
        Check::Accepted {
            missed_events: false
        }
    );
}

#[test]
fn a_replayed_stream_continues_the_old_sequence() {
    let mut session = Session::new(Some(ResumeState {
        connection_id: "c1".to_owned(),
        sequence_number: 40,
    }));
    assert_eq!(
        session.check(&frame("posted", 40)),
        Check::Accepted {
            missed_events: false
        }
    );
    // The same id again (a lossless resume that still sent hello) is not a loss.
    let mut same = Session::new(Some(ResumeState {
        connection_id: "c1".to_owned(),
        sequence_number: 0,
    }));
    assert_eq!(
        same.check(&hello("c1")),
        Check::Accepted {
            missed_events: false
        }
    );
}
