//! `get.php`, `/{M3U_FILE_NAME}` and `xmltv.php`: streamed, rewritten and
//! cached on disk.
//!
//! The first request streams to the player and writes the cache file at the
//! same time ("tee"), so a big EPG is never loaded into memory and the player
//! gets the first byte right away. Requests for the same key that arrive
//! meanwhile wait for that download instead of starting their own. A download
//! the player aborts is not cached.

use std::{
    collections::{BTreeMap, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::{io::AsyncWriteExt, sync::OwnedMutexGuard};
use tokio_util::io::ReaderStream;

use crate::{AppState, Shared, api, auth::Params, stream};

#[derive(Default)]
pub struct Locks(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl Locks {
    fn get(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.0
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .clone()
    }
}

/// `get.php` with the client's parameters (type, output, ...).
pub async fn get_php(State(st): State<Shared>, params: Params) -> Response {
    if !st.authorized(
        params.get("username").unwrap_or(""),
        params.get("password").unwrap_or(""),
    ) {
        return api::unauthorized();
    }
    let ttl = Duration::from_secs(st.cfg.m3u_cache_hours * 3600);
    playlist(&st, params.without_credentials(), ttl).await
}

/// `/{M3U_FILE_NAME}` (default `/iptv.m3u`): get.php with the `M3U_URL`
/// parameters.
pub async fn m3u_file(State(st): State<Shared>, params: Params) -> Response {
    if !st.authorized(
        params.get("username").unwrap_or(""),
        params.get("password").unwrap_or(""),
    ) {
        return api::unauthorized();
    }
    let p: BTreeMap<String, String> = st.cfg.m3u_params.iter().cloned().collect();
    let ttl = Duration::from_secs(st.cfg.m3u_cache_hours * 3600);
    playlist(&st, p, ttl).await
}

async fn playlist(st: &Shared, params: BTreeMap<String, String>, ttl: Duration) -> Response {
    let url = st.provider_url("get.php", &Params(params.clone().into_iter().collect()));
    let key = format!("get.php?{params:?}");
    tracing::info!("get.php {:?}", params);
    cached(st, &key, ttl, url, "audio/x-mpegurl").await
}

/// `xmltv.php`
pub async fn xmltv(State(st): State<Shared>, params: Params) -> Response {
    if !st.authorized(
        params.get("username").unwrap_or(""),
        params.get("password").unwrap_or(""),
    ) {
        return api::unauthorized();
    }
    let p = params.without_credentials();
    let url = st.provider_url("xmltv.php", &Params(p.clone().into_iter().collect()));
    let ttl = Duration::from_secs(st.cfg.xmltv_cache_minutes * 60);
    tracing::info!("xmltv.php");
    cached(
        &st,
        &format!("xmltv.php?{p:?}"),
        ttl,
        url,
        "application/xml",
    )
    .await
}

fn cache_path(st: &AppState, key: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    st.cfg.cache_dir.join(format!("{:016x}", h.finish()))
}

fn fresh(path: &PathBuf, ttl: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < ttl)
}

async fn cached(
    st: &Shared,
    key: &str,
    ttl: Duration,
    url: url::Url,
    ctype: &'static str,
) -> Response {
    let path = cache_path(st, key);
    let use_cache = !ttl.is_zero();

    let lock = if use_cache {
        if fresh(&path, ttl) {
            return serve_file(&path, ctype).await;
        }
        let guard = st.cache_locks.get(key).lock_owned().await;
        // Someone else may have just finished the download.
        if fresh(&path, ttl) {
            drop(guard);
            return serve_file(&path, ctype).await;
        }
        Some(guard)
    } else {
        None
    };

    let resp = match stream::open(st, &st.http, Method::GET, url, None).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let status = resp.status();
    if !status.is_success() {
        return api::rewritten(st, resp);
    }

    let body = st.rewriter.rewrite_stream(resp.bytes_stream());
    let body: Body = match lock {
        Some(guard) => match tee(body, path, guard).await {
            Ok(s) => Body::from_stream(s),
            Err((e, body)) => {
                tracing::warn!("cache disabled for this request: {e}");
                Body::from_stream(body)
            }
        },
        None => Body::from_stream(body),
    };
    let mut r = Response::new(body);
    r.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    r
}

async fn serve_file(path: &PathBuf, ctype: &'static str) -> Response {
    match tokio::fs::File::open(path).await {
        Ok(f) => {
            let len = f.metadata().await.map(|m| m.len()).ok();
            let mut r = Response::new(Body::from_stream(ReaderStream::with_capacity(f, 64 * 1024)));
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
            if let Some(l) = len {
                r.headers_mut()
                    .insert(header::CONTENT_LENGTH, HeaderValue::from(l));
            }
            r
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

type Tee<S> = std::pin::Pin<Box<S>>;

/// Pass `body` through while writing it to `<path>.part`; rename to `path`
/// when the body completes. On error or early drop the part file is removed.
async fn tee<S, E>(
    body: S,
    path: PathBuf,
    guard: OwnedMutexGuard<()>,
) -> Result<impl Stream<Item = Result<Bytes, E>> + Send + 'static, (std::io::Error, Tee<S>)>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let body = Box::pin(body);
    if let Err(e) = tokio::fs::create_dir_all(path.parent().expect("cache file has a parent")).await
    {
        return Err((e, body));
    }
    let part = path.with_extension(format!("part{}", std::process::id()));
    let file = match tokio::fs::File::create(&part).await {
        Ok(f) => f,
        Err(e) => return Err((e, body)),
    };

    let state = TeeState {
        body,
        file: Some(file),
        part: part.clone(),
        path,
        _guard: guard,
        done: false,
    };
    Ok(futures_util::stream::unfold(state, |mut s| async move {
        if s.done {
            return None;
        }
        match s.body.next().await {
            Some(Ok(chunk)) => {
                if let Some(f) = s.file.as_mut()
                    && f.write_all(&chunk).await.is_err()
                {
                    // Disk full or similar: keep serving, stop caching.
                    s.file = None;
                    let _ = tokio::fs::remove_file(&s.part).await;
                }
                Some((Ok(chunk), s))
            }
            Some(Err(e)) => {
                s.file = None;
                let _ = tokio::fs::remove_file(&s.part).await;
                s.done = true;
                Some((Err(e), s))
            }
            None => {
                if let Some(mut f) = s.file.take() {
                    let ok = f.flush().await.is_ok() && f.sync_all().await.is_ok();
                    drop(f);
                    if ok {
                        let _ = tokio::fs::rename(&s.part, &s.path).await;
                    } else {
                        let _ = tokio::fs::remove_file(&s.part).await;
                    }
                }
                None
            }
        }
    }))
}

struct TeeState<S> {
    body: Tee<S>,
    file: Option<tokio::fs::File>,
    part: PathBuf,
    path: PathBuf,
    /// Held until the download ends, so concurrent requests wait for it.
    _guard: OwnedMutexGuard<()>,
    done: bool,
}

impl<S> Drop for TeeState<S> {
    fn drop(&mut self) {
        // Player went away mid-download: don't leave a half file behind.
        if self.file.is_some() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}
