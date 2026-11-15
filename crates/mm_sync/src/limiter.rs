// SPDX-License-Identifier: AGPL-3.0-only

//! Request politeness (MatterLite.md §8.5): a token bucket shared by every request the engine
//! makes, and a global pause after a `429`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::{Instant, sleep_until};

/// Requests per second and burst. The defaults (8/s, burst 20) stay under the server's
/// default rate limit of 10/s with burst 100 (`RateLimitSettings`) when it is enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub per_second: f64,
    pub burst: u32,
}

impl Rate {
    pub const GENTLE: Self = Self {
        per_second: 4.0,
        burst: 10,
    };
    pub const NORMAL: Self = Self {
        per_second: 8.0,
        burst: 20,
    };
    /// No client limit; the server's still applies (429s pause everything).
    pub const FAST: Self = Self {
        per_second: f64::INFINITY,
        burst: u32::MAX,
    };
}

impl Default for Rate {
    fn default() -> Self {
        Self::NORMAL
    }
}

#[derive(Debug)]
struct Bucket {
    rate: Rate,
    tokens: f64,
    refilled: Instant,
    paused_until: Option<Instant>,
}

/// Cheap to clone; clones share the bucket.
#[derive(Clone, Debug)]
pub struct Limiter {
    bucket: Arc<Mutex<Bucket>>,
}

impl Limiter {
    pub fn new(rate: Rate) -> Self {
        Self {
            bucket: Arc::new(Mutex::new(Bucket {
                rate,
                tokens: f64::from(rate.burst),
                refilled: Instant::now(),
                paused_until: None,
            })),
        }
    }

    /// Waits for a token (and for a pause to end).
    pub async fn acquire(&self) {
        loop {
            let wait_until = {
                let mut bucket = self.bucket.lock().await;
                let now = Instant::now();
                if let Some(until) = bucket.paused_until {
                    if until > now {
                        Some(until)
                    } else {
                        bucket.paused_until = None;
                        None
                    }
                } else {
                    None
                }
                .or_else(|| {
                    if bucket.rate.per_second.is_infinite() {
                        return None;
                    }
                    let elapsed = now.duration_since(bucket.refilled).as_secs_f64();
                    bucket.tokens = (bucket.tokens + elapsed * bucket.rate.per_second)
                        .min(f64::from(bucket.rate.burst));
                    bucket.refilled = now;
                    if bucket.tokens >= 1.0 {
                        bucket.tokens -= 1.0;
                        None
                    } else {
                        let missing = 1.0 - bucket.tokens;
                        Some(now + Duration::from_secs_f64(missing / bucket.rate.per_second))
                    }
                })
            };
            match wait_until {
                None => return,
                Some(until) => sleep_until(until).await,
            }
        }
    }

    /// The server said `429`: no request for `retry_after` (default 10 s), and the bucket
    /// starts empty after it.
    pub async fn pause(&self, retry_after: Option<Duration>) {
        let mut bucket = self.bucket.lock().await;
        let until = Instant::now() + retry_after.unwrap_or(Duration::from_secs(10));
        bucket.paused_until = Some(bucket.paused_until.map_or(until, |known| known.max(until)));
        bucket.tokens = 0.0;
        bucket.refilled = until;
    }

    pub async fn set_rate(&self, rate: Rate) {
        let mut bucket = self.bucket.lock().await;
        bucket.rate = rate;
        bucket.tokens = bucket.tokens.min(f64::from(rate.burst));
    }
}

#[cfg(test)]
#[path = "limiter_tests.rs"]
mod tests;
