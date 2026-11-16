// SPDX-License-Identifier: AGPL-3.0-only
// Rewritten for MatterLite; warp-terminal `crates/warp_logging/src/{native,rotation}.rs`
// @ 5bb7bcc6a (startup and size-based log rotation) was the reference.

//! Logging to `<state>/logs/matterlite.log`, also to stderr in debug builds.
//!
//! - Each launch moves the previous `matterlite.log` to `matterlite.log.old.0` (older ones shift
//!   up; [`KEPT_LAUNCHES`] are kept).
//! - Within a launch the active file is rotated to `matterlite.log.in_session.N` once it
//!   reaches [`MAX_FILE_BYTES`], so a long session can't fill the disk.
//! - `RUST_LOG` overrides the default filter.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

const LOG_FILE_NAME: &str = "matterlite.log";
/// Previous launches' logs to keep.
pub const KEPT_LAUNCHES: usize = 5;
/// Size at which the active log file is rotated within a launch.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// In-session rotations to keep.
const KEPT_IN_SESSION: usize = 2;
/// Info for our code, warnings for chatty dependencies (GPU backends, shader compilers).
const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// Initializes the global logger. Returns the path of the active log file.
pub fn init(log_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(log_dir)
        .with_context(|| format!("couldn't create log directory {}", log_dir.display()))?;
    rotate_previous_launches(log_dir, LOG_FILE_NAME, KEPT_LAUNCHES)?;
    let writer = RotatingFileWriter::open(log_dir, LOG_FILE_NAME, MAX_FILE_BYTES, KEPT_IN_SESSION)?;
    let target: Box<dyn Write + Send> = if cfg!(debug_assertions) {
        Box::new(Tee(writer, io::stderr()))
    } else {
        Box::new(writer)
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER))
        .target(env_logger::Target::Pipe(target))
        .try_init()
        .context("a logger is already installed")?;
    log_panics();
    Ok(log_dir.join(LOG_FILE_NAME))
}

/// Writes panics (with the thread and a backtrace) to the log before the default hook runs: a
/// release build on Windows has no console, so the default message would be lost.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(
            "panic in thread '{}': {info}\n{backtrace}",
            thread.name().unwrap_or("<unnamed>")
        );
        log::logger().flush();
        previous(info);
    }));
}

/// Moves `<name>` to `<name>.old.0`, shifting older launches up and dropping the oldest.
fn rotate_previous_launches(dir: &Path, name: &str, keep: usize) -> io::Result<()> {
    let old = |index: usize| dir.join(format!("{name}.old.{index}"));
    let active = dir.join(name);
    if !active.exists() || keep == 0 {
        return Ok(());
    }
    let oldest = old(keep - 1);
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }
    for index in (0..keep - 1).rev() {
        if old(index).exists() {
            fs::rename(old(index), old(index + 1))?;
        }
    }
    fs::rename(active, old(0))
}

/// A file that is rotated to `<name>.in_session.N` once `max_bytes` have been written to it.
struct RotatingFileWriter {
    dir: PathBuf,
    name: String,
    max_bytes: u64,
    keep: usize,
    written: u64,
    file: File,
}

impl RotatingFileWriter {
    fn open(dir: &Path, name: &str, max_bytes: u64, keep: usize) -> io::Result<Self> {
        Ok(Self {
            dir: dir.to_path_buf(),
            name: name.to_string(),
            max_bytes,
            keep,
            written: 0,
            file: create_truncated(&dir.join(name))?,
        })
    }

    fn in_session(&self, index: usize) -> PathBuf {
        self.dir.join(format!("{}.in_session.{index}", self.name))
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let active = self.dir.join(&self.name);
        if self.keep > 0 {
            let oldest = self.in_session(self.keep - 1);
            if oldest.exists() {
                fs::remove_file(&oldest)?;
            }
            for index in (0..self.keep - 1).rev() {
                if self.in_session(index).exists() {
                    fs::rename(self.in_session(index), self.in_session(index + 1))?;
                }
            }
            fs::rename(&active, self.in_session(0))?;
        }
        self.file = create_truncated(&active)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Rotate before a write that would cross the limit, but never leave an empty file
        // behind: an oversized first write goes into the fresh file as is.
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let written = self.file.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn create_truncated(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// Writes everything to both writers; errors from the second one are ignored.
struct Tee<A, B>(A, B);

impl<A: Write, B: Write> Write for Tee<A, B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_all(buf)?;
        let _ = self.1.write_all(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.1.flush();
        self.0.flush()
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
