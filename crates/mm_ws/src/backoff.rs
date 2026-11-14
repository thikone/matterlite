// SPDX-License-Identifier: AGPL-3.0-only

use std::time::Duration;

/// Reconnect delays (`webapp/platform/client/src/websocket.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// The delay while failures are few.
    pub min: Duration,
    pub max: Duration,
    /// After this many consecutive failures the delay grows as `min × failures²`.
    pub fails_before_growth: u32,
    /// Up to this much is added at random so clients don't reconnect in lockstep.
    pub jitter: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            min: Duration::from_secs(3),
            max: Duration::from_secs(5 * 60),
            fails_before_growth: 7,
            jitter: Duration::from_secs(2),
        }
    }
}

impl Backoff {
    /// The delay before the next attempt after `fail_count` consecutive failures (at least
    /// 1). `random` in `[0, 1)` scales the jitter.
    pub fn delay(&self, fail_count: u32, random: f64) -> Duration {
        let base = if fail_count > self.fails_before_growth {
            self.min
                .saturating_mul(fail_count.saturating_mul(fail_count))
                .min(self.max)
        } else {
            self.min
        };
        base + self.jitter.mul_f64(random.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
#[path = "backoff_tests.rs"]
mod tests;
