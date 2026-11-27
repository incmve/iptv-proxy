//! End-to-end: player -> proxy -> fake provider.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use iptv_proxy::{AppState, Config, router};

const MOVIE: &[u8] = b"0123456789abcdefghij"; // 20 bytes

#[derive(Default)]
struct Provider {
    flaky_hits: AtomicU32,
}

async fn provider_movie(
    State(p): State<Arc<Provider>>,
    Path((user, pass, id)): Path<(String, String, String)>,
    h: HeaderMap,
) -> Response {
    if user != "pu" || pass != "pp" {
        return StatusCode::FORBIDDEN.into_response();
    }
    if h.contains_key(header::COOKIE) || h.contains_key("x-forwarded-for") {
        return (StatusCode::IM_A_TEAPOT, "leaked header").into_response();
    }
    match id.as_str() {
        "flaky.mkv" => {
            // Two 503s, then OK: the open-retry must hide this.
            if p.flaky_hits.fetch_add(1, Ordering::SeqCst) < 2 {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            (StatusCode::OK, "ok").into_response()
        }
        "redirect.mkv" => Redirect::temporary("/cdn/1.mkv").into_response(),
        "1.mkv" => serve_range(&h),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn serve_range(h: &HeaderMap) -> Response {
    let ua = h
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if let Some(r) = h.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        let start: usize = r
            .trim_start_matches("bytes=")
            .trim_end_matches('-')
            .parse()
            .unwrap();
        let body = MOVIE[start..].to_vec();
        return (
            StatusCode::PARTIAL_CONTENT,
            [
                (
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{}/{}", MOVIE.len() - 1, MOVIE.len()),
                ),
                (header::ACCEPT_RANGES, "bytes".into()),
                (header::HeaderName::from_static("x-seen-ua"), ua),
            ],
            body,
        )
            .into_response();
    }
    ([(header::ACCEPT_RANGES, "bytes")], MOVIE.to_vec()).into_response()
}

async fn spawn(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    a
}

async fn setup() -> (String, reqwest::Client) {
    let prov = Router::new()
        .route("/movie/{u}/{p}/{id}", get(provider_movie))
        .route(
            "/cdn/1.mkv",
            get(|h: HeaderMap| async move { serve_range(&h) }),
        )
        .with_state(Arc::new(Provider::default()));
    let prov_addr = spawn(prov).await;

    let env: HashMap<&str, String> = HashMap::from([
        ("USER", "test".into()),
        ("PASSWORD", "testpassword".into()),
        (
            "M3U_URL",
            format!("http://{prov_addr}/get.php?username=pu&password=pp&type=m3u_plus"),
        ),
    ]);
    let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
    let proxy_addr = spawn(router(Arc::new(AppState::new(cfg).unwrap()))).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    (format!("http://{proxy_addr}"), client)
}

#[tokio::test]
async fn full_movie() {
    let (base, c) = setup().await;
    let r = c
        .get(format!("{base}/movie/test/testpassword/1.mkv"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["accept-ranges"], "bytes");
    assert_eq!(r.bytes().await.unwrap().as_ref(), MOVIE);
}

#[tokio::test]
async fn range_and_user_agent_pass_through_secrets_do_not() {
    let (base, c) = setup().await;
    let r = c
        .get(format!("{base}/movie/test/testpassword/1.mkv"))
        .header("range", "bytes=10-")
        .header("user-agent", "Jellyfin/10")
        .header("cookie", "session=1")
        .header("x-forwarded-for", "192.168.0.10")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.headers()["content-range"], "bytes 10-19/20");
    assert_eq!(r.headers()["x-seen-ua"], "Jellyfin/10");
    assert_eq!(r.bytes().await.unwrap().as_ref(), &MOVIE[10..]);
}

#[tokio::test]
async fn head_returns_headers_only() {
    let (base, c) = setup().await;
    let r = c
        .head(format!("{base}/movie/test/testpassword/1.mkv"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn wrong_proxy_password_is_rejected() {
    let (base, c) = setup().await;
    let r = c
        .get(format!("{base}/movie/test/wrong/1.mkv"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn retries_transient_provider_errors_while_opening() {
    let (base, c) = setup().await;
    let r = c
        .get(format!("{base}/movie/test/testpassword/flaky.mkv"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn follows_provider_redirect_without_exposing_it() {
    let (base, c) = setup().await;
    let r = c
        .get(format!("{base}/movie/test/testpassword/redirect.mkv"))
        .send()
        .await
        .unwrap();
    // The player sees the movie, not a redirect to the provider's CDN.
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("location").is_none());
    assert_eq!(r.bytes().await.unwrap().as_ref(), MOVIE);
}

#[tokio::test]
async fn health_is_ok_without_credentials() {
    let (base, c) = setup().await;
    let r = c.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(r.status(), 200);
}
