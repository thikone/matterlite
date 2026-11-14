// SPDX-License-Identifier: AGPL-3.0-only

//! The writer thread (MatterLite.md §6.1, §8.3): one SQLite connection, writes batched into a
//! transaction every 250 ms or 200 ops, whichever comes first. A batch is all or nothing, so
//! sync cursors written with their data can never claim data the cache doesn't have (§8.7).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use diesel::sqlite::SqliteConnection;

use crate::db;
use crate::error::CacheError;
use crate::ops::Op;

/// How long ops wait for more to join their batch.
pub const BATCH_WINDOW: Duration = Duration::from_millis(250);
/// A batch is written at once when it reaches this size.
pub const BATCH_OPS: usize = 200;

enum Message {
    Ops(Vec<Op>),
    /// Answered once everything sent before it is committed.
    Flush(mpsc::SyncSender<Result<(), String>>),
}

/// Sends ops to the writer thread. Clones share the thread; it ends (after writing what's
/// queued) when the last one is dropped or [`Writer::close`] is called.
#[derive(Clone)]
pub struct Writer {
    sender: Sender<Message>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Writer {
    /// Starts the thread on an open connection (from [`crate::Cache::into_writer`]).
    pub(crate) fn start(mut conn: SqliteConnection) -> Result<Self, CacheError> {
        let (sender, receiver) = mpsc::channel();
        let last_error = Arc::new(Mutex::new(None));
        let errors = last_error.clone();
        let thread = std::thread::Builder::new()
            .name("cache writer".into())
            .spawn(move || run(&mut conn, &receiver, &errors))
            .map_err(|source| CacheError::Io {
                path: "cache writer thread".into(),
                source,
            })?;
        Ok(Self {
            sender,
            thread: Arc::new(Mutex::new(Some(thread))),
            last_error,
        })
    }

    /// Queues ops for the next batch. Fails only when the thread has stopped.
    pub fn send(&self, ops: Vec<Op>) -> Result<(), CacheError> {
        if ops.is_empty() {
            return Ok(());
        }
        self.sender
            .send(Message::Ops(ops))
            .map_err(|_| CacheError::WriterStopped)
    }

    /// Waits until everything sent so far is committed (shutdown, tests, before a cursor
    /// that depends on it). Returns the error of the batch that held them, if it failed.
    pub fn flush(&self) -> Result<(), CacheError> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.sender
            .send(Message::Flush(reply))
            .map_err(|_| CacheError::WriterStopped)?;
        match answer.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) => Err(CacheError::Corrupt(message)),
            Err(_) => Err(CacheError::WriterStopped),
        }
    }

    /// The last failed batch, for diagnostics.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|error| error.clone())
    }

    /// Writes what's queued and stops the thread.
    pub fn close(self) -> Result<(), CacheError> {
        let result = self.flush();
        let thread = self.thread.lock().ok().and_then(|mut thread| thread.take());
        drop(self.sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
        result
    }
}

fn run(conn: &mut SqliteConnection, receiver: &Receiver<Message>, errors: &Mutex<Option<String>>) {
    let mut pending: Vec<Op> = Vec::new();
    let mut flushes: Vec<mpsc::SyncSender<Result<(), String>>> = Vec::new();
    let mut deadline: Option<Instant> = None;
    loop {
        let message = match deadline {
            None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(deadline) => {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
        };
        let disconnected = match message {
            Ok(Message::Ops(ops)) => {
                pending.extend(ops);
                deadline.get_or_insert_with(|| Instant::now() + BATCH_WINDOW);
                false
            }
            Ok(Message::Flush(reply)) => {
                flushes.push(reply);
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        let due = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if pending.len() >= BATCH_OPS || due || !flushes.is_empty() || disconnected {
            let result = if pending.is_empty() {
                Ok(())
            } else {
                let ops = std::mem::take(&mut pending);
                db::apply(conn, &ops).map_err(|error| {
                    let message = error.to_string();
                    log::error!("cache batch of {} ops failed: {message}", ops.len());
                    if let Ok(mut last) = errors.lock() {
                        *last = Some(message.clone());
                    }
                    message
                })
            };
            deadline = None;
            for reply in flushes.drain(..) {
                let _ = reply.send(result.clone());
            }
        }
        if disconnected {
            return;
        }
    }
}
