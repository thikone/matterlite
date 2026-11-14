// SPDX-License-Identifier: AGPL-3.0-only

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cannot open the cache: {0}")]
    Connect(#[from] diesel::ConnectionError),
    #[error("cache query failed: {0}")]
    Query(#[from] diesel::result::Error),
    #[error("the cache is damaged: {0}")]
    Corrupt(String),
    #[error("cache migration failed: {0}")]
    Migration(String),
    #[error("cache path is not valid UTF-8: {}", .0.display())]
    Path(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cached {what} cannot be read: {source}")]
    Decode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("the cache writer has stopped")]
    WriterStopped,
}
