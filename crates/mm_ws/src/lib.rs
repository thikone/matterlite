// SPDX-License-Identifier: AGPL-3.0-only

//! The Mattermost WebSocket client (WebApp.md §7).
//!
//! [`connect`] spawns a task on the current Tokio runtime that keeps one connection open:
//!
//! - authenticates with the Bearer header on the upgrade request (the server only resumes
//!   connections that were authenticated this way) and with the `authentication_challenge`
//!   action;
//! - checks every event's `seq`; on a gap it closes with 4001 and reconnects with
//!   `connection_id` and `sequence_number`, so the server replays what was missed;
//! - reports [`WsOutput::MissedEvents`] when the server answers a resume with a new connection
//!   id: events were lost and the caller must run a full resync;
//! - pings every 30 s and reconnects (4000) when a ping goes unanswered;
//! - backs off between attempts like the webapp: 3 s, then `3 s × n²` after 7 failures, at most
//!   5 min, plus up to 2 s of jitter.
//!
//! The task ends when [`WsHandle::close`] is called, every handle is dropped, the output
//! receiver is dropped, or the server rejects the token.

mod backoff;
mod connection;
mod session;
mod tls;

pub use backoff::Backoff;
pub use connection::{DisconnectReason, WsConfig, WsHandle, WsOutput, connect};
pub use session::ResumeState;
