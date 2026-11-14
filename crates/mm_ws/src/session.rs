// SPDX-License-Identifier: AGPL-3.0-only

use mm_model::ws::EventFrame;
use url::Url;

/// Where to resume the event stream: the server's connection id and the next expected event
/// `seq`. Keep it across reconnects (and app restarts) to get missed events replayed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResumeState {
    pub connection_id: String,
    pub sequence_number: i64,
}

/// The outcome of checking one event.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// In sequence. `missed_events`: this is a `hello` with a new connection id although we
    /// had one, so the server could not resume and events were lost.
    Accepted { missed_events: bool },
    /// Out of sequence: reconnect to get the missing events replayed.
    Gap { expected: i64, got: i64 },
}

/// Sequence tracking across connections (`websocket.ts` `onmessage`).
#[derive(Debug, Default)]
pub(crate) struct Session {
    state: ResumeState,
}

impl Session {
    pub(crate) fn new(resume: Option<ResumeState>) -> Self {
        Self {
            state: resume.unwrap_or_default(),
        }
    }

    pub(crate) fn is_resuming(&self) -> bool {
        !self.state.connection_id.is_empty()
    }

    /// The URL for the next connection attempt.
    pub(crate) fn connect_url(&self, base: &Url, last_close_code: Option<u16>) -> Url {
        let mut url = base.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("connection_id", &self.state.connection_id)
                .append_pair("sequence_number", &self.state.sequence_number.to_string());
            if let Some(code) = last_close_code {
                query.append_pair("disconnect_err_code", &code.to_string());
            }
        }
        url
    }

    pub(crate) fn check(&mut self, frame: &EventFrame) -> Check {
        let mut missed_events = false;
        if frame.event == "hello" {
            let connection_id = frame
                .data
                .get("connection_id")
                .and_then(|id| id.as_str())
                .unwrap_or_default();
            if self.is_resuming() && connection_id != self.state.connection_id {
                missed_events = true;
                self.state.sequence_number = 0;
            }
            self.state.connection_id = connection_id.to_owned();
        }
        if frame.seq != self.state.sequence_number {
            return Check::Gap {
                expected: self.state.sequence_number,
                got: frame.seq,
            };
        }
        self.state.sequence_number = frame.seq + 1;
        Check::Accepted { missed_events }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
