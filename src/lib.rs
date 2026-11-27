//! iptv-proxy (Rust): an Xtream Codes reverse proxy.
//!
//! See docs/DESIGN.md for scope, decisions and milestones.

pub mod api;
pub mod auth;
pub mod cache;
pub mod catchup;
pub mod config;
pub mod headers;
pub mod live;
pub mod rewrite;
pub mod shared;
pub mod stream;
pub mod throttle;
pub mod upstream;

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use subtle::ConstantTimeEq;
use url::Url;

pub use config::Config;

pub struct AppState {
    pub cfg: Config,
    /// Follows provider redirects (streams, API).
    pub http: reqwest::Client,
    pub rewriter: rewrite::Rewriter,
    /// Load-balancer host per HLS channel.
    pub hls: live::HlsOrigins,
    /// Shared live channels.
    pub shared: shared::Registry,
    /// One download at a time per cached playlist/EPG.
    pub cache_locks: cache::Locks,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(cfg: Config) -> reqwest::Result<Self> {
        let http = upstream::build(&cfg.provider_user_agent)?;
        let rewriter = rewrite::Rewriter::new(&cfg);
        Ok(Self {
            cfg,
            http,
            rewriter,
            hls: Default::default(),
            shared: Default::default(),
            cache_locks: Default::default(),
        })
    }

    /// Constant-time check of the proxy credentials a client sent.
    pub fn authorized(&self, user: &str, pass: &str) -> bool {
        let u = user.as_bytes().ct_eq(self.cfg.user.expose().as_bytes());
        let p = pass.as_bytes().ct_eq(self.cfg.password.expose().as_bytes());
        bool::from(u & p)
    }

    /// `<provider>/<kind>/<provider user>/<provider pass>/<rest...>`, percent-encoded.
    pub fn provider_stream_url(&self, kind: &str, rest: &[&str]) -> Url {
        let mut u = self.cfg.xtream_base_url.clone();
        {
            let mut seg = u.path_segments_mut().expect("http(s) base URL");
            seg.pop_if_empty();
            if !kind.is_empty() {
                seg.push(kind);
            }
            seg.push(self.cfg.xtream_user.expose());
            seg.push(self.cfg.xtream_password.expose());
            seg.extend(rest);
        }
        u
    }

    pub fn redact(&self, s: &str) -> String {
        upstream::redact(
            s,
            &[
                self.cfg.xtream_user.expose(),
                self.cfg.xtream_password.expose(),
                self.cfg.password.expose(),
            ],
        )
    }
}

pub fn router(state: Shared) -> Router {
    let m3u_file = format!("/{}", state.cfg.m3u_file_name.trim_start_matches('/'));
    Router::new()
        .route("/get.php", get(cache::get_php).post(cache::get_php))
        .route("/xmltv.php", get(cache::xmltv).post(cache::xmltv))
        .route(&m3u_file, get(cache::m3u_file))
        .route("/health", get(health))
        .route(
            "/player_api.php",
            get(api::player_api).post(api::player_api),
        )
        .route("/movie/{user}/{pass}/{id}", get(movie).head(movie))
        .route("/series/{user}/{pass}/{id}", get(series).head(series))
        .route("/live/{user}/{pass}/{id}", get(live::live).head(live::live))
        .route(
            "/{user}/{pass}/{id}",
            get(live::live_alt).head(live::live_alt),
        )
        .route("/hls/{token}/{chunk}", get(live::hls).head(live::hls))
        .route(
            "/hlsr/{token}/{user}/{pass}/{channel}/{hash}/{chunk}",
            get(live::hlsr).head(live::hlsr),
        )
        .route("/play/{token}/{kind}", get(live::play).head(live::play))
        .route(
            "/timeshift/{user}/{pass}/{duration}/{start}/{id}",
            get(catchup::path_style).head(catchup::path_style),
        )
        .route(
            "/timeshift.php",
            get(catchup::query_style).post(catchup::query_style),
        )
        .route(
            "/streaming/timeshift.php",
            get(catchup::query_style).post(catchup::query_style),
        )
        .with_state(state)
}

/// Liveness only: never touches the provider, so the Docker healthcheck can't
/// cause provider traffic or stream glitches (lesson from the Go version).
async fn health() -> &'static str {
    "ok"
}

async fn movie(
    State(st): State<Shared>,
    method: Method,
    Path((user, pass, id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    vod(&st, "movie", method, &user, &pass, &id, &headers).await
}

async fn series(
    State(st): State<Shared>,
    method: Method,
    Path((user, pass, id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    vod(&st, "series", method, &user, &pass, &id, &headers).await
}

async fn vod(
    st: &AppState,
    kind: &str,
    method: Method,
    user: &str,
    pass: &str,
    id: &str,
    headers: &HeaderMap,
) -> Response {
    if !st.authorized(user, pass) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::info!("{method} {kind} {id}");
    let url = st.provider_stream_url(kind, &[id]);
    match stream::open(st, &st.http, method.clone(), url, Some(headers)).await {
        Ok(resp) => {
            let throttle = (st.cfg.vod_throttle_kbps > 0).then_some(st.cfg.vod_throttle_kbps);
            stream::passthrough(resp, &method, throttle)
        }
        Err(e) => e.into_response(),
    }
}
