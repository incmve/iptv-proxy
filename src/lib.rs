//! iptv-proxy (Rust): an Xtream Codes reverse proxy.
//!
//! See docs/DESIGN.md for scope, decisions and milestones.

pub mod config;
pub mod headers;
pub mod stream;
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
    pub http: reqwest::Client,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(cfg: Config) -> reqwest::Result<Self> {
        let http = upstream::build(&cfg.provider_user_agent)?;
        Ok(Self { cfg, http })
    }

    /// Constant-time check of the proxy credentials a client sent.
    pub fn authorized(&self, user: &str, pass: &str) -> bool {
        let u = user.as_bytes().ct_eq(self.cfg.user.expose().as_bytes());
        let p = pass.as_bytes().ct_eq(self.cfg.password.expose().as_bytes());
        bool::from(u & p)
    }

    /// `<provider>/<kind>/<provider user>/<provider pass>/<id>`, percent-encoded.
    pub fn provider_stream_url(&self, kind: &str, id: &str) -> Url {
        let mut u = self.cfg.xtream_base_url.clone();
        {
            let mut seg = u.path_segments_mut().expect("http(s) base URL");
            seg.pop_if_empty();
            if !kind.is_empty() {
                seg.push(kind);
            }
            seg.push(self.cfg.xtream_user.expose());
            seg.push(self.cfg.xtream_password.expose());
            seg.push(id);
        }
        u
    }
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/movie/{user}/{pass}/{id}", get(movie).head(movie))
        .route("/series/{user}/{pass}/{id}", get(series).head(series))
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
    let url = st.provider_stream_url(kind, id);
    stream::relay(st, method, url, headers).await
}
