//! Live streams and HLS.
//!
//! MPEG-TS channels (`/live/u/p/123.ts`, `/u/p/123`) are relayed (or shared,
//! see `shared.rs`). HLS playlists (`.m3u8`) are fetched, segment URIs are
//! rewritten to point at the proxy, and segments are relayed.
//!
//! Xtream providers usually answer `/live/u/p/123.m3u8` with a 302 to a
//! load-balancer host whose playlist lists `/hls/<token>/123_<n>.ts`. The
//! proxy remembers that host per channel, so `/hls/...` segment requests go
//! to the right place. A playlist answered directly with 200 works too (the
//! Go version sent an empty body in that case).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use url::Url;

use crate::{AppState, Shared, headers, stream};

/// Forget a channel's load-balancer host after this long without use.
const ORIGIN_TTL: Duration = Duration::from_secs(60 * 60);
/// Max size of an HLS playlist we rewrite.
const MAX_PLAYLIST: usize = 4 * 1024 * 1024;

/// channel id -> (origin that served its playlist, last used).
#[derive(Default)]
pub struct HlsOrigins(Mutex<HashMap<String, (Url, Instant)>>);

impl HlsOrigins {
    fn set(&self, channel: &str, origin: Url) {
        let mut m = self.0.lock().unwrap();
        let now = Instant::now();
        m.retain(|_, (_, t)| now.duration_since(*t) < ORIGIN_TTL);
        m.insert(channel.to_string(), (origin, now));
    }

    fn get(&self, channel: &str) -> Option<Url> {
        let mut m = self.0.lock().unwrap();
        let e = m.get_mut(channel)?;
        if e.1.elapsed() >= ORIGIN_TTL {
            m.remove(channel);
            return None;
        }
        e.1 = Instant::now();
        Some(e.0.clone())
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// `/live/{u}/{p}/{id}`
pub async fn live(
    State(st): State<Shared>,
    method: Method,
    Path((user, pass, id)): Path<(String, String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !st.authorized(&user, &pass) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    channel(&st, "live", method, &id, query.as_deref(), &headers).await
}

/// `/{u}/{p}/{id}` (live without the `/live` prefix)
pub async fn live_alt(
    State(st): State<Shared>,
    method: Method,
    Path((user, pass, id)): Path<(String, String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !st.authorized(&user, &pass) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    channel(&st, "", method, &id, query.as_deref(), &headers).await
}

async fn channel(
    st: &Shared,
    kind: &str,
    method: Method,
    id: &str,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let mut url = st.provider_stream_url(kind, &[id]);
    // Load balancers often put a token in the query: pass it on.
    url.set_query(query);
    if id.ends_with(".m3u8") {
        let ch = id.trim_end_matches(".m3u8");
        tracing::info!("live hls {ch}");
        return playlist(st, url, Some(ch), headers).await;
    }
    if method == Method::GET && st.cfg.share_live && is_channel_id(id) {
        tracing::info!("live ts {id}");
        return crate::shared::serve(st, id, url, headers).await;
    }
    // A segment next to a playlist (`123_45.ts`): ask the host that served
    // that channel's playlist. Otherwise (HEAD, sharing off): the provider.
    let url = match id.split_once('_') {
        Some((ch, _)) if !is_channel_id(id) => {
            let mut u = st.hls_origin(ch);
            u.set_path(url.path());
            u.set_query(url.query());
            u
        }
        _ => url,
    };
    stream::relay(st, method, url, headers).await
}

/// `123` or `123.ts`: a live channel (not a segment like `123_45.ts`).
pub fn is_channel_id(id: &str) -> bool {
    let n = id.strip_suffix(".ts").unwrap_or(id);
    !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
}

/// `/hls/{token}/{chunk}`: chunk is `<channel>_<n>.ts` (or a nested playlist).
pub async fn hls(
    State(st): State<Shared>,
    method: Method,
    Path((token, chunk)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let channel = chunk
        .split('_')
        .next()
        .unwrap_or("")
        .trim_end_matches(".m3u8")
        .to_string();
    let mut url = st.hls_origin(&channel);
    url.path_segments_mut()
        .expect("http url")
        .clear()
        .extend(["hls", &token, &chunk]);
    url.set_query(query.as_deref());
    segment_or_playlist(&st, method, url, &chunk, Some(&channel), &headers).await
}

/// `/hlsr/{token}/{u}/{p}/{channel}/{hash}/{chunk}`
pub async fn hlsr(
    State(st): State<Shared>,
    method: Method,
    Path((token, user, pass, channel, hash, chunk)): Path<(
        String,
        String,
        String,
        String,
        String,
        String,
    )>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !st.authorized(&user, &pass) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut url = st.hls_origin(&channel);
    url.path_segments_mut().expect("http url").clear().extend([
        "hlsr",
        &token,
        st.cfg.xtream_user.expose(),
        st.cfg.xtream_password.expose(),
        &channel,
        &hash,
        &chunk,
    ]);
    url.set_query(query.as_deref());
    segment_or_playlist(&st, method, url, &chunk, Some(&channel), &headers).await
}

/// `/play/{token}/{type}`
pub async fn play(
    State(st): State<Shared>,
    method: Method,
    Path((token, kind)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let mut url = st.cfg.xtream_base_url.clone();
    url.path_segments_mut()
        .expect("http url")
        .clear()
        .extend(["play", &token, &kind]);
    url.set_query(query.as_deref());
    segment_or_playlist(&st, method, url, &kind, None, &headers).await
}

pub async fn segment_or_playlist(
    st: &Shared,
    method: Method,
    url: Url,
    name: &str,
    channel: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    if name.ends_with(".m3u8") {
        playlist(st, url, channel, headers).await
    } else {
        stream::relay(st, method, url, headers).await
    }
}

/// Fetch an HLS playlist (following redirects), remember where it came from,
/// and rewrite its URIs to the proxy.
pub async fn playlist(
    st: &Shared,
    url: Url,
    channel: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let resp = match stream::open(st, &st.http, Method::GET, url, Some(headers)).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let status = resp.status();
    let final_url = resp.url().clone();
    let mut hdrs = headers::client_response(resp.headers());
    hdrs.remove(header::CONTENT_LENGTH);

    if !status.is_success() {
        let mut r = Response::new(Body::from_stream(resp.bytes_stream()));
        *r.status_mut() = status;
        *r.headers_mut() = hdrs;
        return r;
    }

    let mut origin = final_url.clone();
    origin.set_path("/");
    origin.set_query(None);
    if let Some(ch) = channel {
        st.hls.set(ch, origin);
    }

    let body = match read_limited(resp, MAX_PLAYLIST).await {
        Some(b) => b,
        None => return stream::Unreachable.into_response(),
    };
    let text = String::from_utf8_lossy(&body);
    let out = rewrite_playlist(st, &text, &final_url);

    let mut r = Response::new(Body::from(out));
    *r.status_mut() = status;
    *r.headers_mut() = hdrs;
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

/// Rewrite every URI line of a playlist:
/// - on the provider host or the playlist's own host -> proxy-relative path
///   (credentials swapped), so the player asks the proxy;
/// - on any other host -> left as is (credentials swapped if present).
pub fn rewrite_playlist(st: &AppState, text: &str, playlist_url: &Url) -> String {
    let base = &st.cfg.xtream_base_url;
    let mut out = String::with_capacity(text.len() + 256);
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
        } else if let Ok(abs) = playlist_url.join(t) {
            if same_origin(&abs, base) || same_origin(&abs, playlist_url) {
                let mut p = abs.path().to_string();
                if let Some(q) = abs.query() {
                    p.push('?');
                    p.push_str(q);
                }
                out.push_str(&String::from_utf8_lossy(
                    &st.rewriter.rewrite_all(p.as_bytes()),
                ));
            } else {
                out.push_str(&String::from_utf8_lossy(
                    &st.rewriter.rewrite_all(t.as_bytes()),
                ));
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

async fn read_limited(mut resp: reqwest::Response, max: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        buf.extend_from_slice(&chunk);
        if buf.len() > max {
            return None;
        }
    }
    Some(buf)
}

impl AppState {
    /// The host that served this channel's playlist, else the provider.
    pub fn hls_origin(&self, channel: &str) -> Url {
        self.hls.get(channel).unwrap_or_else(|| {
            let mut u = self.cfg.xtream_base_url.clone();
            u.set_path("/");
            u
        })
    }
}
