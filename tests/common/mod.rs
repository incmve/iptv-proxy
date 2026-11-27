//! A fake Xtream Codes provider and a proxy in front of it.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use bytes::Bytes;
use iptv_proxy::{AppState, Config, router};

pub const PU: &str = "provuser";
pub const PP: &str = "provpass";
pub const USER: &str = "test";
pub const PASS: &str = "testpassword";

#[derive(Default)]
pub struct Provider {
    pub port: AtomicUsize,
    pub get_php_hits: AtomicUsize,
    pub xmltv_hits: AtomicUsize,
    /// Live TS upstream connections currently open, and opened in total.
    pub live_open: AtomicUsize,
    pub live_opened_total: AtomicUsize,
    pub last_query: Mutex<HashMap<String, String>>,
    pub last_live_ua: Mutex<String>,
}

impl Provider {
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port.load(Ordering::SeqCst))
    }
    /// Same server, different host name: plays the provider's load balancer.
    pub fn lb_origin(&self) -> String {
        format!("http://localhost:{}", self.port.load(Ordering::SeqCst))
    }
}

pub struct Env {
    pub base: String,
    pub prov: Arc<Provider>,
    pub client: reqwest::Client,
}

fn creds_ok(q: &HashMap<String, String>) -> bool {
    q.get("username").map(String::as_str) == Some(PU)
        && q.get("password").map(String::as_str) == Some(PP)
}

async fn player_api(
    State(p): State<Arc<Provider>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    *p.last_query.lock().unwrap() = q.clone();
    if !creds_ok(&q) {
        return (StatusCode::OK, r#"{"user_info":{"auth":0}}"#).into_response();
    }
    let o = p.origin().replace('/', "\\/");
    let port = p.port.load(Ordering::SeqCst);
    let json = |s: String| ([(header::CONTENT_TYPE, "application/json")], s).into_response();
    match q.get("action").map(String::as_str) {
        None => json(format!(
            r#"{{"user_info":{{"username":"{PU}","password":"{PP}","message":"hi","auth":1,"status":"Active","exp_date":"1900000000","is_trial":"0","active_cons":"0","created_at":"1600000000","max_connections":"2","allowed_output_formats":["m3u8","ts"]}},"server_info":{{"url":"127.0.0.1","port":"{port}","https_port":"","server_protocol":"http","rtmp_port":"","timezone":"Europe\/Amsterdam","timestamp_now":1764260000,"time_now":"2026-11-27 18:00:00","process":true}}}}"#
        )),
        Some("get_vod_streams") => json(format!(
            r#"[{{"num":1,"name":"Film","stream_id":1,"stream_icon":"{o}\/images\/a.png","tmdb":"603","direct_source":"{o}\/movie\/{PU}\/{PP}\/1.mkv"}}]"#
        )),
        Some("get_series_info") => {
            json(r#"{"info":{"name":"Show","tmdb":"1399"},"episodes":{}}"#.into())
        }
        Some("get_live_streams") => {
            let item = format!(
                r#"{{"name":"Ch","stream_id":5,"direct_source":"{o}\/live\/{PU}\/{PP}\/5.ts"}}"#
            );
            json(format!("[{}]", vec![item; 5000].join(",")))
        }
        Some(other) => json(format!(r#"{{"echo":"{other}"}}"#)),
    }
}

async fn get_php(
    State(p): State<Arc<Provider>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !creds_ok(&q) {
        return StatusCode::FORBIDDEN.into_response();
    }
    p.get_php_hits.fetch_add(1, Ordering::SeqCst);
    let o = p.origin();
    let mut s = format!(
        "#EXTM3U\n#PARAMS type={} output={}\n",
        q.get("type").cloned().unwrap_or_default(),
        q.get("output").cloned().unwrap_or_default()
    );
    for i in 0..2000 {
        s.push_str(&format!(
            "#EXTINF:-1 tvg-logo=\"{o}/images/{i}.png\" group-title=\"G\",Channel {i}\n{o}/live/{PU}/{PP}/{i}.ts\n"
        ));
    }
    s.into_response()
}

async fn xmltv(
    State(p): State<Arc<Provider>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !creds_ok(&q) {
        return StatusCode::FORBIDDEN.into_response();
    }
    p.xmltv_hits.fetch_add(1, Ordering::SeqCst);
    let mut s = String::from("<?xml version=\"1.0\"?><tv>");
    for i in 0..20000 {
        s.push_str(&format!(
            "<programme channel=\"c{i}\"><title>Show {i}</title></programme>"
        ));
    }
    s.push_str("</tv>");
    ([(header::CONTENT_TYPE, "application/xml")], s).into_response()
}

struct LiveGuard(Arc<Provider>);
impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.live_open.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Endless MPEG-TS: 188-byte packets, sync byte 0x47, a running counter.
fn endless_ts(p: Arc<Provider>) -> Body {
    p.live_open.fetch_add(1, Ordering::SeqCst);
    p.live_opened_total.fetch_add(1, Ordering::SeqCst);
    let g = LiveGuard(p);
    let s = futures_util::stream::unfold((g, 0u32), |(g, n)| async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut chunk = Vec::with_capacity(188 * 10);
        for i in 0..10u32 {
            let mut pkt = [0xffu8; 188];
            pkt[0] = 0x47;
            pkt[4..8].copy_from_slice(&(n * 10 + i).to_be_bytes());
            chunk.extend_from_slice(&pkt);
        }
        // Deliberately split off-packet, like a real network would.
        let cut = 100;
        let b = Bytes::from(chunk);
        let parts = vec![Ok::<_, std::io::Error>(b.slice(..cut)), Ok(b.slice(cut..))];
        Some((futures_util::stream::iter(parts), (g, n + 1)))
    })
    .flatten();
    Body::from_stream(s)
}

use futures_util::StreamExt;

async fn live(
    State(p): State<Arc<Provider>>,
    Path((u, pw, id)): Path<(String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Response {
    if u != PU || pw != PP {
        return StatusCode::FORBIDDEN.into_response();
    }
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    match id.as_str() {
        // HLS via 302 to a load balancer (the classic Xtream way).
        "10.m3u8" if host.starts_with("127.0.0.1") => {
            Redirect::temporary(&format!("{}/live/{PU}/{PP}/10.m3u8?token=abc", p.lb_origin())).into_response()
        }
        "10.m3u8" => {
            assert_eq!(q.get("token").map(String::as_str), Some("abc"));
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\n/hls/tok123/10_1.ts\n#EXTINF:4,\n/hls/tok123/10_2.ts\n"
                .into_response()
        }
        // HLS answered directly with 200 (the Go version sent an empty body).
        "20.m3u8" => format!(
            "#EXTM3U\n#EXTINF:4,\n20_1.ts\n#EXTINF:4,\n{}/hls/tok/20_2.ts\n#EXTINF:4,\nhttp://cdn.other/x/20_3.ts\n",
            p.origin()
        )
        .into_response(),
        // Relative segment requested next to the 200 playlist.
        "20_1.ts" => "seg 20_1".into_response(),
        "404.ts" => StatusCode::NOT_FOUND.into_response(),
        _ => {
            *p.last_live_ua.lock().unwrap() =
                h.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            ([(header::CONTENT_TYPE, "video/mp2t")], endless_ts(p)).into_response()
        }
    }
}

async fn hls_seg(Path((token, chunk)): Path<(String, String)>, h: HeaderMap) -> String {
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    format!(
        "seg {token} {chunk} via {}",
        host.split(':').next().unwrap_or("")
    )
}

async fn timeshift(
    Path((u, pw, dur, start, id)): Path<(String, String, String, String, String)>,
) -> Response {
    if u != PU || pw != PP {
        return StatusCode::FORBIDDEN.into_response();
    }
    format!("timeshift {dur} {start} {id}").into_response()
}

async fn timeshift_php(Query(q): Query<HashMap<String, String>>) -> Response {
    if !creds_ok(&q) {
        return StatusCode::FORBIDDEN.into_response();
    }
    format!(
        "timeshift.php stream={} start={} duration={}",
        q["stream"], q["start"], q["duration"]
    )
    .into_response()
}

async fn movie(Path((u, pw, _id)): Path<(String, String, String)>) -> Response {
    if u != PU || pw != PP {
        return StatusCode::FORBIDDEN.into_response();
    }
    vec![0u8; 2_000_000].into_response()
}

pub async fn spawn(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    a
}

pub async fn setup() -> Env {
    setup_with(&[]).await
}

pub async fn setup_with(extra: &[(&str, &str)]) -> Env {
    let prov = Arc::new(Provider::default());
    let app = Router::new()
        .route("/player_api.php", get(player_api))
        .route("/get.php", get(get_php))
        .route("/xmltv.php", get(xmltv))
        .route("/live/{u}/{p}/{id}", get(live))
        .route("/{u}/{p}/{id}", get(live))
        .route("/hls/{token}/{chunk}", get(hls_seg))
        .route("/timeshift/{u}/{p}/{dur}/{start}/{id}", get(timeshift))
        .route("/streaming/timeshift.php", get(timeshift_php))
        .route("/timeshift.php", get(timeshift_php))
        .route("/movie/{u}/{p}/{id}", get(movie))
        .route(
            "/play/{token}/{t}",
            get(|Path((t, ty)): Path<(String, String)>| async move { format!("play {t} {ty}") }),
        )
        .with_state(prov.clone());
    let addr = spawn(app).await;
    prov.port.store(addr.port() as usize, Ordering::SeqCst);

    let cache = std::env::temp_dir().join(format!("iptv-proxy-test-{}", addr.port()));
    let mut env: HashMap<String, String> = HashMap::from([
        ("USER".into(), USER.into()),
        ("PASSWORD".into(), PASS.into()),
        ("HOSTNAME".into(), "iptvproxy".into()),
        ("PORT".into(), "8080".into()),
        ("CACHE_DIR".into(), cache.to_string_lossy().into_owned()),
        (
            "M3U_URL".into(),
            format!(
                "http://{addr}/get.php?username={PU}&password={PP}&type=m3u_plus&output=mpegts"
            ),
        ),
    ]);
    for (k, v) in extra {
        env.insert(k.to_string(), v.to_string());
    }
    let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
    let proxy = spawn(router(Arc::new(AppState::new(cfg).unwrap()))).await;

    Env {
        base: format!("http://{proxy}"),
        prov,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
    }
}

/// Poll until `f` is true (max ~2 s).
pub async fn eventually(f: impl Fn() -> bool) -> bool {
    for _ in 0..100 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
