// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[tokio::test(start_paused = true)]
async fn the_burst_is_free_then_the_rate_applies() {
    let limiter = Limiter::new(Rate {
        per_second: 4.0,
        burst: 3,
    });
    let start = Instant::now();
    for _ in 0..3 {
        limiter.acquire().await;
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
    for _ in 0..4 {
        limiter.acquire().await;
    }
    // Four more at 4/s: one second.
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(999) && elapsed < Duration::from_millis(1100),
        "{elapsed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_429_pauses_everything() {
    let limiter = Limiter::new(Rate::NORMAL);
    limiter.acquire().await;
    limiter.pause(Some(Duration::from_secs(5))).await;
    let start = Instant::now();
    limiter.acquire().await;
    assert!(start.elapsed() >= Duration::from_secs(5));
    // A shorter pause doesn't cut a longer one short.
    limiter.pause(Some(Duration::from_secs(8))).await;
    limiter.pause(Some(Duration::from_secs(1))).await;
    let start = Instant::now();
    limiter.acquire().await;
    assert!(start.elapsed() >= Duration::from_secs(8));
}

#[tokio::test(start_paused = true)]
async fn fast_has_no_client_limit() {
    let limiter = Limiter::new(Rate::FAST);
    let start = Instant::now();
    for _ in 0..1000 {
        limiter.acquire().await;
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
}
