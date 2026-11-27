//! Credentials and parameters from query string or form body.
//!
//! Xtream clients send `username`/`password` (and `action`, `stream`, ...)
//! either in the query string (GET) or as a form body (POST).

use std::collections::BTreeMap;

use axum::{
    extract::{FromRequest, Request},
    http::{Method, StatusCode, header},
};
use url::form_urlencoded;

/// Max size of a form body we accept (it only carries a few parameters).
const MAX_FORM: usize = 64 * 1024;

/// All parameters of the request: query string first, then form body.
/// The same key in both keeps both values (query first), as PHP would see
/// them merged in `$_REQUEST`.
#[derive(Debug, Default, Clone)]
pub struct Params(pub Vec<(String, String)>);

impl Params {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// Everything except the client's credentials, deduplicated by key
    /// (first value wins), in a stable order.
    pub fn without_credentials(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (k, v) in &self.0 {
            if k != "username" && k != "password" {
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        out
    }
}

impl<S: Send + Sync> FromRequest<S> for Params {
    type Rejection = StatusCode;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let mut params: Vec<(String, String)> = req
            .uri()
            .query()
            .map(|q| form_urlencoded::parse(q.as_bytes()).into_owned().collect())
            .unwrap_or_default();

        let is_form = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
        // Some apps POST a form without a content type; accept any POST body.
        if req.method() == Method::POST || is_form {
            let body = axum::body::Bytes::from_request(req, state)
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            if body.len() > MAX_FORM {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            params.extend(form_urlencoded::parse(&body).into_owned());
        }
        Ok(Params(params))
    }
}
