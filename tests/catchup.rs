//! Milestone 6: catch-up in both URL styles.
mod common;
use common::*;

#[tokio::test]
async fn path_style() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/timeshift/{USER}/{PASS}/60/2026-11-27:20-00/5.ts",
            e.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.text().await.unwrap(),
        "timeshift 60 2026-11-27:20-00 5.ts"
    );
}

#[tokio::test]
async fn query_style_get_both_paths() {
    let e = setup().await;
    for path in ["streaming/timeshift.php", "timeshift.php"] {
        let r = e
            .client
            .get(format!(
                "{}/{path}?username={USER}&password={PASS}&stream=5&start=2026-11-27:20-00&duration=60",
                e.base
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert_eq!(
            r.text().await.unwrap(),
            "timeshift.php stream=5 start=2026-11-27:20-00 duration=60"
        );
    }
}

#[tokio::test]
async fn query_style_post() {
    let e = setup().await;
    let r = e
        .client
        .post(format!("{}/streaming/timeshift.php", e.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!(
            "username={USER}&password={PASS}&stream=5&start=2026-11-27:20-00&duration=60"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.text().await.unwrap(),
        "timeshift.php stream=5 start=2026-11-27:20-00 duration=60"
    );
}

#[tokio::test]
async fn catchup_needs_proxy_credentials() {
    let e = setup().await;
    let r = e
        .client
        .get(format!("{}/timeshift/{USER}/bad/60/x/5.ts", e.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = e
        .client
        .get(format!(
            "{}/streaming/timeshift.php?username={USER}&password=bad&stream=5",
            e.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}
