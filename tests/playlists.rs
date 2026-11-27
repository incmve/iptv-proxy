//! Milestone 5: get.php, /iptv.m3u, xmltv.php (streamed, rewritten, cached).
mod common;
use std::sync::atomic::Ordering;

use common::*;

#[tokio::test]
async fn get_php_is_rewritten_and_cached() {
    let e = setup().await;
    let url = format!(
        "{}/get.php?username={USER}&password={PASS}&type=m3u_plus&output=ts",
        e.base
    );
    let first = e
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        first.starts_with("#EXTM3U\n#PARAMS type=m3u_plus output=ts\n"),
        "client params not passed on"
    );
    assert!(
        !first.contains(PU) && !first.contains(PP),
        "provider credentials leaked"
    );
    assert_eq!(
        first
            .matches(&format!("http://iptvproxy:8080/live/{USER}/{PASS}/"))
            .count(),
        2000
    );
    // Logos stay on the provider.
    assert!(first.contains(&format!("{}/images/7.png", e.prov.origin())));

    let second = e
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(
        e.prov.get_php_hits.load(Ordering::SeqCst),
        1,
        "second request was not served from cache"
    );

    // Different parameters = different cache entry.
    let other = format!(
        "{}/get.php?username={USER}&password={PASS}&type=m3u_plus&output=m3u8",
        e.base
    );
    e.client
        .get(&other)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(e.prov.get_php_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn m3u_file_uses_m3u_url_parameters() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/iptv.m3u?username={USER}&password={PASS}",
            e.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    assert!(
        text.contains("#PARAMS type=m3u_plus output=mpegts"),
        "{}",
        &text[..60]
    );
}

#[tokio::test]
async fn concurrent_requests_download_once() {
    let e = setup().await;
    let url = format!(
        "{}/get.php?username={USER}&password={PASS}&type=m3u",
        e.base
    );
    let reqs = (0..5).map(|_| {
        let c = e.client.clone();
        let u = url.clone();
        tokio::spawn(async move { c.get(&u).send().await.unwrap().text().await.unwrap() })
    });
    let bodies: Vec<String> = futures_util::future::join_all(reqs)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert!(bodies.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(e.prov.get_php_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn aborted_download_is_not_cached() {
    let e = setup().await;
    let url = format!("{}/xmltv.php?username={USER}&password={PASS}", e.base);
    // Read a little, then hang up.
    let mut r = e.client.get(&url).send().await.unwrap();
    let _ = r.chunk().await.unwrap();
    drop(r);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let full = e
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(full.ends_with("</tv>"));
    assert_eq!(
        e.prov.xmltv_hits.load(Ordering::SeqCst),
        2,
        "half download was served from cache"
    );

    // Now it is cached.
    let again = e
        .client
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(full, again);
    assert_eq!(e.prov.xmltv_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn xmltv_cache_can_be_switched_off() {
    let e = setup_with(&[("XMLTV_CACHE_MINUTES", "0")]).await;
    let url = format!("{}/xmltv.php?username={USER}&password={PASS}", e.base);
    for _ in 0..2 {
        assert!(
            e.client
                .get(&url)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
                .ends_with("</tv>")
        );
    }
    assert_eq!(e.prov.xmltv_hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn playlists_need_proxy_credentials() {
    let e = setup().await;
    for p in ["get.php", "xmltv.php", "iptv.m3u"] {
        let r = e
            .client
            .get(format!("{}/{p}?username={USER}&password=bad", e.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}
