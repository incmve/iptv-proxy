//! `player_api.php`: passed through as-is, except the login answer.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use url::Url;

use crate::{Shared, auth::Params, headers, stream};

/// Max size of a login answer we parse (it's a few hundred bytes).
const MAX_LOGIN: usize = 1024 * 1024;

pub async fn player_api(State(st): State<Shared>, params: Params) -> Response {
    if !st.authorized(
        params.get("username").unwrap_or(""),
        params.get("password").unwrap_or(""),
    ) {
        return unauthorized();
    }
    let action = params.get("action").unwrap_or("").to_string();
    let url = st.provider_url("player_api.php", &params);

    let resp = match stream::open(&st, &st.http, Method::GET, url, None).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    tracing::info!(
        "player_api {} -> {}",
        if action.is_empty() { "login" } else { &action },
        resp.status()
    );

    if action.is_empty() {
        login(&st, resp).await
    } else {
        rewritten(&st, resp)
    }
}

/// What a real panel answers for bad credentials.
pub fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"user_info":{"auth":0}}"#,
    )
        .into_response()
}

/// Relay a text answer with provider URLs/credentials swapped for the proxy's.
pub fn rewritten(st: &Shared, resp: reqwest::Response) -> Response {
    let status = resp.status();
    let mut hdrs = headers::client_response(resp.headers());
    // The length changes when URLs are rewritten.
    hdrs.remove(header::CONTENT_LENGTH);
    let body = Body::from_stream(st.rewriter.rewrite_stream(resp.bytes_stream()));
    let mut out = Response::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = hdrs;
    out
}

/// The login answer (no `action`): `user_info` gets the proxy credentials and
/// `server_info` the proxy address, in the format real panels use (`url` is
/// a bare hostname, ports are strings). Everything else is kept as sent.
async fn login(st: &Shared, resp: reqwest::Response) -> Response {
    let status = resp.status();
    let ctype = resp.headers().get(header::CONTENT_TYPE).cloned();
    let body = match read_limited(resp, MAX_LOGIN).await {
        Some(b) => b,
        None => return stream::Unreachable.into_response(),
    };

    let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
        // Not JSON (provider error page): relay it, minus credentials.
        let mut r = Response::new(Body::from(st.rewriter.rewrite_all(&body)));
        *r.status_mut() = status;
        if let Some(ct) = ctype {
            r.headers_mut().insert(header::CONTENT_TYPE, ct);
        }
        return r;
    };

    let cfg = &st.cfg;
    if let Some(ui) = v.get_mut("user_info").and_then(Value::as_object_mut) {
        if ui.contains_key("username") {
            ui.insert("username".into(), cfg.user.expose().into());
        }
        if ui.contains_key("password") {
            ui.insert("password".into(), cfg.password.expose().into());
        }
    }
    if let Some(si) = v.get_mut("server_info").and_then(Value::as_object_mut) {
        let port = cfg.advertised_port.to_string();
        si.insert("url".into(), cfg.hostname.clone().into());
        si.insert("port".into(), port.clone().into());
        si.insert("https_port".into(), port.clone().into());
        si.insert("rtmp_port".into(), port.into());
        si.insert(
            "server_protocol".into(),
            (if cfg.https { "https" } else { "http" }).into(),
        );
    }

    let mut r = Response::new(Body::from(serde_json::to_vec(&v).unwrap_or_default()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r
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

impl crate::AppState {
    /// `<provider>/<path>?username=..&password=..&<client params>`.
    pub fn provider_url(&self, path: &str, params: &Params) -> Url {
        let mut u = self.cfg.xtream_base_url.clone();
        u.set_path(path);
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("username", self.cfg.xtream_user.expose());
            q.append_pair("password", self.cfg.xtream_password.expose());
            for (k, v) in params.without_credentials() {
                q.append_pair(&k, &v);
            }
        }
        u
    }
}
