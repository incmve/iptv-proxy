//! Opening provider requests (with retry) and relaying them to the client.

use std::time::Duration;

use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use url::Url;

use crate::{AppState, headers};

/// Attempts to open a request before giving up (tuliprox default: 3).
const OPEN_ATTEMPTS: u32 = 3;
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_FACTOR: f64 = 1.5;
/// Never wait longer than this for a provider's Retry-After.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

/// The provider could not be reached (after retries).
#[derive(Debug)]
pub struct Unreachable;

impl IntoResponse for Unreachable {
    fn into_response(self) -> Response {
        (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response()
    }
}

/// Send a request to the provider. Retries only while *opening* (connect
/// errors, 5xx); once the response is returned, nothing is retried. A 4xx
/// (for example "max connections reached") is returned at once.
///
/// `headers`: the client's headers (filtered), or `None` for the proxy's own
/// API calls, which use the fixed provider User-Agent.
pub async fn open(
    state: &AppState,
    client: &reqwest::Client,
    method: Method,
    url: Url,
    headers: Option<&HeaderMap>,
) -> Result<reqwest::Response, Unreachable> {
    let fwd = headers.map(headers::upstream_request).unwrap_or_default();
    let mut delay = BACKOFF_START;

    for attempt in 1..=OPEN_ATTEMPTS {
        let result = client
            .request(method.clone(), url.clone())
            .headers(fwd.clone())
            .send()
            .await;
        let wait = match result {
            Ok(resp) if !resp.status().is_server_error() || attempt == OPEN_ATTEMPTS => {
                return Ok(resp);
            }
            Ok(resp) => {
                tracing::warn!(
                    "provider answered {} for {} (attempt {attempt}/{OPEN_ATTEMPTS})",
                    resp.status(),
                    state.redact(url.as_str())
                );
                retry_after(resp.headers()).unwrap_or(delay)
            }
            Err(e) => {
                tracing::warn!(
                    "provider unreachable for {} (attempt {attempt}/{OPEN_ATTEMPTS}): {}",
                    state.redact(url.as_str()),
                    state.redact(&e.to_string())
                );
                if attempt == OPEN_ATTEMPTS {
                    break;
                }
                delay
            }
        };
        tokio::time::sleep(wait).await;
        delay = delay.mul_f64(BACKOFF_FACTOR);
    }
    Err(Unreachable)
}

/// Open `url` with the client's headers and relay the answer byte for byte.
pub async fn relay(
    state: &AppState,
    method: Method,
    url: Url,
    client_headers: &HeaderMap,
) -> Response {
    match open(
        state,
        &state.http,
        method.clone(),
        url,
        Some(client_headers),
    )
    .await
    {
        Ok(resp) => passthrough(resp, &method, None),
        Err(e) => e.into_response(),
    }
}

/// Turn a provider response into a client response, unchanged.
/// `throttle_kbps`: optional rate cap on the body (VOD throttle).
pub fn passthrough(
    resp: reqwest::Response,
    method: &Method,
    throttle_kbps: Option<u64>,
) -> Response {
    let status = resp.status();
    let hdrs = headers::client_response(resp.headers());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        match throttle_kbps {
            Some(kbps) => Body::from_stream(crate::throttle::throttle(resp.bytes_stream(), kbps)),
            None => Body::from_stream(resp.bytes_stream()),
        }
    };
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = hdrs;
    out
}

/// `Retry-After: <seconds>` (the HTTP-date form is rare for providers; ignored).
fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let secs: u64 = h
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}
