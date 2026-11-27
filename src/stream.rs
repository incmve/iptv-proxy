//! Relaying a provider stream to the client, byte for byte.

use std::time::Duration;

use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use url::Url;

use crate::{AppState, headers, upstream::redact};

/// Attempts to open a stream before giving up (tuliprox default: 3).
const OPEN_ATTEMPTS: u32 = 3;
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_FACTOR: f64 = 1.5;
/// Never wait longer than this for a provider's Retry-After.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(5);

/// Open `url` at the provider and relay it.
///
/// Retries only while *opening* (connect errors, 5xx). Once bytes flow, a
/// drop ends the response and the player reconnects, as in tuliprox.
pub async fn relay(
    state: &AppState,
    method: Method,
    url: Url,
    client_headers: &HeaderMap,
) -> Response {
    let fwd = headers::upstream_request(client_headers);
    let mut delay = BACKOFF_START;

    for attempt in 1..=OPEN_ATTEMPTS {
        let result = state
            .http
            .request(method.clone(), url.clone())
            .headers(fwd.clone())
            .send()
            .await;

        let retry_wait = match result {
            Ok(resp) if !resp.status().is_server_error() || attempt == OPEN_ATTEMPTS => {
                return into_client_response(resp, &method);
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
        tokio::time::sleep(retry_wait).await;
        delay = delay.mul_f64(BACKOFF_FACTOR);
    }

    (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response()
}

fn into_client_response(resp: reqwest::Response, method: &Method) -> Response {
    let status = resp.status();
    let hdrs = headers::client_response(resp.headers());
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(resp.bytes_stream())
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

impl AppState {
    pub fn redact(&self, s: &str) -> String {
        redact(
            s,
            &[
                self.cfg.xtream_user.expose(),
                self.cfg.xtream_password.expose(),
                self.cfg.password.expose(),
            ],
        )
    }
}
