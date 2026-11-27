//! Milestone 8: optional VOD throttle.
mod common;
use std::time::Instant;

use common::*;

#[tokio::test]
async fn vod_unthrottled_by_default() {
    let e = setup().await;
    let t = Instant::now();
    let b = e
        .client
        .get(format!("{}/movie/{USER}/{PASS}/1.mkv", e.base))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(b.len(), 2_000_000);
    assert!(t.elapsed().as_secs_f64() < 1.0);
}

#[tokio::test]
async fn vod_throttle_caps_rate() {
    // 2400 kbps = 300 kB/s; burst 4 s = 1.2 MB; 2 MB movie -> ~2.7 s.
    let e = setup_with(&[("VOD_THROTTLE_KBPS", "2400")]).await;
    let t = Instant::now();
    let b = e
        .client
        .get(format!("{}/movie/{USER}/{PASS}/1.mkv", e.base))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(b.len(), 2_000_000);
    let s = t.elapsed().as_secs_f64();
    assert!((2.2..4.0).contains(&s), "took {s}s");
}
