//! Milestones 3 (live + HLS) and 4 (shared live streams).
mod common;
use std::{sync::atomic::Ordering, time::Duration};

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------- HLS ----------

#[tokio::test]
async fn hls_via_302_to_load_balancer() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/live/{USER}/{PASS}/10.m3u8", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let pl = r.text().await.unwrap();
    assert!(pl.contains("\n/hls/tok123/10_1.ts\n"), "{pl}");
    assert!(!pl.contains(PU) && !pl.contains(PP));

    // The segment must be fetched from the load balancer, not the provider.
    let seg = e
        .client
        .get(format!("{}/hls/tok123/10_1.ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        seg.text().await.unwrap(),
        "seg tok123 10_1.ts via localhost"
    );
}

#[tokio::test]
async fn hls_answered_with_200_keeps_its_body() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/live/{USER}/{PASS}/20.m3u8", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let pl = r.text().await.unwrap();
    // Relative and provider-absolute segments become proxy paths; other hosts stay.
    assert!(
        pl.contains(&format!("\n/live/{USER}/{PASS}/20_1.ts\n")),
        "{pl}"
    );
    assert!(pl.contains("\n/hls/tok/20_2.ts\n"), "{pl}");
    assert!(pl.contains("\nhttp://cdn.other/x/20_3.ts\n"), "{pl}");

    let seg = e
        .client
        .get(format!("{}/live/{USER}/{PASS}/20_1.ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(seg.text().await.unwrap(), "seg 20_1");
    let seg = e
        .client
        .get(format!("{}/hls/tok/20_2.ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(seg.text().await.unwrap(), "seg tok 20_2.ts via 127.0.0.1");
}

#[tokio::test]
async fn hls_needs_proxy_credentials() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/live/{USER}/wrong/10.m3u8", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn play_token_is_relayed() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/play/abc/ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "play abc ts");
}

#[tokio::test]
async fn provider_error_is_passed_to_player() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/live/{USER}/{PASS}/404.ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

// ---------- shared live TS ----------

/// A raw player: sends the request and returns the socket after the headers.
async fn player(base: &str, path: &str) -> (tokio::net::TcpStream, Vec<u8>) {
    let addr = base.trim_start_matches("http://");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nUser-Agent: TiviMate/5\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        let n = s.read(&mut tmp).await.unwrap();
        assert!(n > 0, "connection closed before headers");
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let body = buf.split_off(i + 4);
            assert!(
                buf.starts_with(b"HTTP/1.1 200"),
                "{}",
                String::from_utf8_lossy(&buf)
            );
            return (s, body);
        }
    }
}

/// Read until `n` body bytes are available (chunked framing is stripped
/// crudely by searching for TS packets afterwards).
async fn read_more(s: &mut tokio::net::TcpStream, mut have: Vec<u8>, n: usize) -> Vec<u8> {
    let mut tmp = [0u8; 16384];
    while have.len() < n {
        let k = tokio::time::timeout(Duration::from_secs(2), s.read(&mut tmp))
            .await
            .unwrap()
            .unwrap();
        assert!(k > 0);
        have.extend_from_slice(&tmp[..k]);
    }
    have
}

/// Packet counters (bytes 4..8 of each TS packet) found in a raw body.
fn counters(raw: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 188 <= raw.len() {
        if raw[i] == 0x47 && raw[i + 1] == 0xff && raw[i + 2] == 0xff && raw[i + 3] == 0xff {
            out.push(u32::from_be_bytes(raw[i + 4..i + 8].try_into().unwrap()));
            i += 188;
        } else {
            i += 1;
        }
    }
    out
}

#[tokio::test]
async fn two_players_share_one_provider_connection() {
    let e = setup().await;
    let (mut a, body_a) = player(&e.base, &format!("/live/{USER}/{PASS}/5.ts")).await;
    let body_a = read_more(&mut a, body_a, 20_000).await;
    // Second player on the same channel, via the other URL style.
    let (mut b, body_b) = player(&e.base, &format!("/{USER}/{PASS}/5.ts")).await;
    let body_b = read_more(&mut b, body_b, 20_000).await;

    assert_eq!(
        e.prov.live_opened_total.load(Ordering::SeqCst),
        1,
        "second player opened its own upstream"
    );
    assert_eq!(e.prov.live_open.load(Ordering::SeqCst), 1);
    assert_eq!(*e.prov.last_live_ua.lock().unwrap(), "TiviMate/5");

    // The joiner's data starts on a packet boundary and is gap-free.
    let cb = counters(&body_b);
    assert!(cb.len() > 50);
    assert!(
        cb.windows(2).all(|w| w[1] == w[0] + 1),
        "gap or duplicate in joiner stream"
    );
    let ca = counters(&body_a);
    assert!(
        ca.windows(2).all(|w| w[1] == w[0] + 1),
        "gap or duplicate in first stream"
    );
}

#[tokio::test]
async fn one_leaves_other_keeps_playing_last_leaves_closes_upstream() {
    let e = setup().await;
    let (mut a, ba) = player(&e.base, &format!("/live/{USER}/{PASS}/6.ts")).await;
    let _ = read_more(&mut a, ba, 5_000).await;
    let (mut b, bb) = player(&e.base, &format!("/live/{USER}/{PASS}/6.ts")).await;
    let _ = read_more(&mut b, bb, 5_000).await;

    drop(a);
    // B still gets fresh data after A left.
    let more = read_more(&mut b, Vec::new(), 20_000).await;
    assert!(counters(&more).len() > 50);
    assert_eq!(e.prov.live_open.load(Ordering::SeqCst), 1);

    drop(b);
    assert!(
        eventually(|| e.prov.live_open.load(Ordering::SeqCst) == 0).await,
        "upstream left open"
    );
}

#[tokio::test]
async fn different_channels_use_their_own_connection() {
    let e = setup().await;
    let (mut a, ba) = player(&e.base, &format!("/live/{USER}/{PASS}/7.ts")).await;
    let _ = read_more(&mut a, ba, 2_000).await;
    let (mut b, bb) = player(&e.base, &format!("/live/{USER}/{PASS}/8.ts")).await;
    let _ = read_more(&mut b, bb, 2_000).await;
    assert_eq!(e.prov.live_open.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn stalled_player_does_not_block_the_other() {
    let e = setup().await;
    // A connects and then never reads again.
    let (stalled, _) = player(&e.base, &format!("/live/{USER}/{PASS}/9.ts")).await;
    let (mut b, bb) = player(&e.base, &format!("/live/{USER}/{PASS}/9.ts")).await;
    // B keeps receiving well past what fits in A's socket buffers + lag window.
    let got = read_more(&mut b, bb, 2_000_000).await;
    assert!(counters(&got).len() > 5_000);
    drop(stalled);
}

#[tokio::test]
async fn rejoin_after_everyone_left_opens_a_new_upstream() {
    let e = setup().await;
    let (a, _) = player(&e.base, &format!("/live/{USER}/{PASS}/11.ts")).await;
    drop(a);
    assert!(eventually(|| e.prov.live_open.load(Ordering::SeqCst) == 0).await);
    let (mut b, bb) = player(&e.base, &format!("/live/{USER}/{PASS}/11.ts")).await;
    let _ = read_more(&mut b, bb, 2_000).await;
    assert_eq!(e.prov.live_opened_total.load(Ordering::SeqCst), 2);
    assert_eq!(e.prov.live_open.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sharing_can_be_switched_off() {
    let e = setup_with(&[("SHARE_LIVE_STREAMS", "false")]).await;
    let (mut a, ba) = player(&e.base, &format!("/live/{USER}/{PASS}/5.ts")).await;
    let _ = read_more(&mut a, ba, 2_000).await;
    let (mut b, bb) = player(&e.base, &format!("/live/{USER}/{PASS}/5.ts")).await;
    let _ = read_more(&mut b, bb, 2_000).await;
    assert_eq!(e.prov.live_opened_total.load(Ordering::SeqCst), 2);
}
