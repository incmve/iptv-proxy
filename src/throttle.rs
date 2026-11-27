//! Optional VOD throttle (`VOD_THROTTLE_KBPS`, 0 = off).
//!
//! Some players download a movie at full line speed. Providers can see that as
//! scraping. This caps the average rate per stream (tuliprox `throttle_kbps`).
//! The first few seconds' worth of data passes unthrottled so playback and
//! seeking start fast.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::time::Instant;

/// Seconds of data allowed as an initial burst.
const BURST_SECS: u64 = 4;

pub fn throttle<S, E>(input: S, kbps: u64) -> impl Stream<Item = Result<Bytes, E>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let bytes_per_sec = (kbps.max(1) * 1000 / 8) as f64;
    let burst = bytes_per_sec * BURST_SECS as f64;
    let start = Instant::now();
    let mut sent: f64 = 0.0;

    input.then(move |item| {
        let wait = if let Ok(chunk) = &item {
            sent += chunk.len() as f64;
            let due = Duration::from_secs_f64(((sent - burst).max(0.0)) / bytes_per_sec);
            let elapsed = start.elapsed();
            due.checked_sub(elapsed)
        } else {
            None
        };
        async move {
            if let Some(w) = wait {
                tokio::time::sleep(w).await;
            }
            item
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn caps_average_rate_after_burst() {
        // 800 kbps = 100 kB/s; burst = 400 kB. Send 1 MB in 10 kB chunks.
        let chunks: Vec<Result<Bytes, ()>> = (0..100)
            .map(|_| Ok(Bytes::from(vec![0u8; 10_000])))
            .collect();
        let start = Instant::now();
        let n = throttle(futures_util::stream::iter(chunks), 800)
            .count()
            .await;
        assert_eq!(n, 100);
        // (1 MB - 400 kB) / 100 kB/s = 6 s.
        let took = start.elapsed().as_secs_f64();
        assert!((5.8..6.3).contains(&took), "took {took}s");
    }
}
