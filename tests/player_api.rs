mod common;
use common::*;
use serde_json::Value;

#[tokio::test]
async fn login_points_at_proxy_and_keeps_other_fields() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/player_api.php?username={USER}&password={PASS}",
            e.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["user_info"]["username"], USER);
    assert_eq!(v["user_info"]["password"], PASS);
    assert_eq!(v["user_info"]["max_connections"], "2");
    assert_eq!(v["user_info"]["message"], "hi");
    assert_eq!(v["server_info"]["url"], "iptvproxy");
    assert_eq!(v["server_info"]["port"], "8080");
    assert_eq!(v["server_info"]["server_protocol"], "http");
    assert_eq!(v["server_info"]["timezone"], "Europe/Amsterdam");
    assert_eq!(v["server_info"]["process"], true);
}

#[tokio::test]
async fn login_via_post_form() {
    let e = setup().await;
    let r = e
        .client
        .post(format!("{}/player_api.php", e.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("username={USER}&password={PASS}"))
        .send()
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["user_info"]["username"], USER);
}

#[tokio::test]
async fn post_action_in_form_is_honoured() {
    let e = setup().await;
    let r = e
        .client
        .post(format!("{}/player_api.php", e.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!(
            "username={USER}&password={PASS}&action=get_series_info&series_id=7"
        ))
        .send()
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["info"]["tmdb"], "1399");
    assert_eq!(e.prov.last_query.lock().unwrap()["series_id"], "7");
}

#[tokio::test]
async fn wrong_credentials_get_auth_0() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/player_api.php?username={USER}&password=nope",
            e.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["user_info"]["auth"], 0);
}

#[tokio::test]
async fn unknown_fields_survive_and_stream_urls_are_rewritten() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/player_api.php?username={USER}&password={PASS}&action=get_vod_streams",
            e.base
        ))
        .send()
        .await
        .unwrap();
    let text = r.text().await.unwrap();
    assert!(
        !text.contains(PU) && !text.contains(PP),
        "provider credentials leaked: {text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    // The field the Go structs dropped:
    assert_eq!(v[0]["tmdb"], "603");
    assert_eq!(
        v[0]["direct_source"],
        format!("http://iptvproxy:8080/movie/{USER}/{PASS}/1.mkv")
    );
    // Logos stay on the provider.
    assert_eq!(
        v[0]["stream_icon"],
        format!("{}/images/a.png", e.prov.origin())
    );
}

#[tokio::test]
async fn unknown_actions_are_passed_through_not_answered_with_login() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/player_api.php?username={USER}&password={PASS}&action=get_brand_new_thing",
            e.base
        ))
        .send()
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(v["echo"], "get_brand_new_thing");
}

#[tokio::test]
async fn large_answer_streams_and_is_fully_rewritten() {
    let e = setup().await;
    let r = e
        .client
        .get(format!(
            "{}/player_api.php?username={USER}&password={PASS}&action=get_live_streams",
            e.base
        ))
        .send()
        .await
        .unwrap();
    let text = r.text().await.unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 5000);
    assert!(!text.contains(PP));
    assert_eq!(text.matches("iptvproxy:8080").count(), 5000);
}

#[tokio::test]
async fn provider_gets_its_own_credentials_and_client_params() {
    let e = setup().await;
    e.client
        .get(format!("{}/player_api.php?username={USER}&password={PASS}&action=get_vod_streams&category_id=12", e.base))
        .send()
        .await
        .unwrap();
    let q = e.prov.last_query.lock().unwrap().clone();
    assert_eq!(q["username"], PU);
    assert_eq!(q["password"], PP);
    assert_eq!(q["category_id"], "12");
}
