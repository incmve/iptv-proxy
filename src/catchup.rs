//! Catch-up / timeshift, in both URL styles apps use:
//! - path:  `/timeshift/{u}/{p}/{duration}/{start}/{id}`
//! - query: `/timeshift.php` and `/streaming/timeshift.php`
//!   (`username`, `password`, `stream`, `start`, `duration`; GET or POST)
//!
//! The Go version only had the path style.

use axum::{
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{Shared, api, auth::Params, live, stream};

pub async fn path_style(
    State(st): State<Shared>,
    method: Method,
    Path((user, pass, duration, start, id)): Path<(String, String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if !st.authorized(&user, &pass) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tracing::info!("timeshift {id} start {start} duration {duration}");
    let url = st.provider_stream_url("timeshift", &[&duration, &start, &id]);
    if id.ends_with(".m3u8") {
        return live::playlist(&st, url, None, &headers).await;
    }
    stream::relay(&st, method, url, &headers).await
}

pub async fn query_style(
    State(st): State<Shared>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    params: Params,
) -> Response {
    if !st.authorized(
        params.get("username").unwrap_or(""),
        params.get("password").unwrap_or(""),
    ) {
        return api::unauthorized();
    }
    tracing::info!(
        "timeshift.php stream {} start {} duration {}",
        params.get("stream").unwrap_or("?"),
        params.get("start").unwrap_or("?"),
        params.get("duration").unwrap_or("?")
    );
    // Same path at the provider as the client used.
    let url = st.provider_url(uri.path().trim_start_matches('/'), &params);
    // POST from the client becomes GET upstream (the parameters are in the URL).
    let method = if method == Method::HEAD {
        Method::HEAD
    } else {
        Method::GET
    };
    stream::relay(&st, method, url, &headers).await
}
