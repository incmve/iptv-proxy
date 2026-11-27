//! Shared live MPEG-TS streams: players on the same live channel share one
//! provider connection.
//!
//! - The first player opens the provider stream; later players join it.
//! - A joiner starts from a small buffer of recent data, cut at an MPEG-TS
//!   packet boundary (188 bytes, sync byte 0x47).
//! - A player that falls too far behind is dropped (it reconnects and joins
//!   again); it never slows the others down.
//! - The provider connection closes as soon as the last player leaves.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use tokio::sync::broadcast;
use url::Url;

use crate::{Shared, headers, stream};

/// Recent data kept for joiners.
pub const RING_BYTES: usize = 1024 * 1024;
/// Chunks a player may fall behind before it is dropped.
pub const LAG_CHUNKS: usize = 1024;
/// How often the provider task checks whether anyone is still watching when
/// no data arrives.
const IDLE_CHECK: Duration = Duration::from_millis(250);

const TS_PACKET: usize = 188;
const TS_SYNC: u8 = 0x47;

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Channel>>>>;

#[derive(Default)]
pub struct Registry {
    slots: Mutex<HashMap<String, Slot>>,
}

impl Registry {
    fn slot(&self, id: &str) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Live channels with an open provider connection, and their viewers.
    pub fn active(&self) -> Vec<(String, usize)> {
        let slots: Vec<(String, Slot)> = self
            .slots
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        slots
            .into_iter()
            .filter_map(|(k, s)| {
                let g = s.try_lock().ok()?;
                let ch = g.as_ref()?;
                let open = !ch.state.lock().unwrap().closed;
                open.then(|| (k, ch.tx.receiver_count()))
            })
            .collect()
    }
}

struct Channel {
    id: String,
    tx: broadcast::Sender<Bytes>,
    state: Mutex<State>,
    status: StatusCode,
    headers: HeaderMap,
}

#[derive(Default)]
struct State {
    ring: VecDeque<Bytes>,
    ring_len: usize,
    closed: bool,
}

impl Channel {
    /// Join a running channel: recent data (packet-aligned) + a receiver,
    /// taken atomically so no byte is lost or duplicated.
    fn join(&self) -> Option<(Bytes, broadcast::Receiver<Bytes>)> {
        let st = self.state.lock().unwrap();
        if st.closed {
            return None;
        }
        let mut buf = BytesMut::with_capacity(st.ring_len);
        for c in &st.ring {
            buf.extend_from_slice(c);
        }
        let rx = self.tx.subscribe();
        drop(st);
        let buf = buf.freeze();
        let start = ts_align(&buf);
        Some((buf.slice(start..), rx))
    }

    fn push(&self, chunk: Bytes) {
        let mut st = self.state.lock().unwrap();
        st.ring_len += chunk.len();
        st.ring.push_back(chunk.clone());
        while st.ring_len > RING_BYTES {
            match st.ring.pop_front() {
                Some(old) => st.ring_len -= old.len(),
                None => break,
            }
        }
        // Under the same lock as `join`, so a joiner gets either this chunk in
        // its snapshot or from its receiver, never both or neither.
        let _ = self.tx.send(chunk);
    }

    fn close(&self) {
        let mut st = self.state.lock().unwrap();
        st.closed = true;
        st.ring.clear();
        st.ring_len = 0;
    }
}

/// First offset in `buf` where MPEG-TS packets start: a sync byte that is
/// confirmed by the next one or two packets. Empty buffer -> 0.
pub fn ts_align(buf: &[u8]) -> usize {
    let n = buf.len();
    for i in 0..TS_PACKET.min(n) {
        if buf[i] != TS_SYNC {
            continue;
        }
        let ok = (1..=2).all(|k| {
            let j = i + k * TS_PACKET;
            j >= n || buf[j] == TS_SYNC
        });
        if ok {
            return i;
        }
    }
    0
}

/// Serve a live MPEG-TS channel, sharing the provider connection.
pub async fn serve(st: &Shared, id: &str, url: Url, client_headers: &HeaderMap) -> Response {
    let key = id.strip_suffix(".ts").unwrap_or(id).to_string();
    let slot = st.shared.slot(&key);
    // Held while opening, so two players starting the same channel at the
    // same moment don't open two provider connections.
    let mut guard = slot.lock().await;

    if let Some(ch) = guard.as_ref()
        && let Some((prefix, rx)) = ch.join()
    {
        let ch = ch.clone();
        drop(guard);
        tracing::info!(
            "live {key}: joined shared stream ({} viewers)",
            ch.tx.receiver_count()
        );
        return respond(&ch, prefix, rx);
    }

    let resp = match stream::open(st, &st.http, Method::GET, url, Some(client_headers)).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    if !resp.status().is_success() {
        // Provider refused (e.g. connection limit): pass its answer on as-is.
        return stream::passthrough(resp, &Method::GET, None);
    }

    let mut hdrs = headers::client_response(resp.headers());
    hdrs.remove(header::CONTENT_LENGTH);
    let (tx, rx) = broadcast::channel(LAG_CHUNKS);
    let ch = Arc::new(Channel {
        id: key.clone(),
        tx,
        state: Mutex::default(),
        status: resp.status(),
        headers: hdrs,
    });
    *guard = Some(ch.clone());
    drop(guard);

    tokio::spawn(pump(ch.clone(), resp, st.clone()));
    respond(&ch, Bytes::new(), rx)
}

/// Read the provider stream and hand it to every viewer. Ends (and closes the
/// provider connection) when the stream ends or nobody is watching.
async fn pump(ch: Arc<Channel>, resp: reqwest::Response, st: Shared) {
    let mut body = resp.bytes_stream();
    let mut tick = tokio::time::interval(IDLE_CHECK);
    loop {
        if ch.tx.receiver_count() == 0 {
            tracing::info!("live {}: last viewer left, closing provider stream", ch.id);
            break;
        }
        tokio::select! {
            next = body.next() => match next {
                Some(Ok(chunk)) => ch.push(chunk),
                Some(Err(e)) => {
                    tracing::warn!("live {}: provider stream error: {}", ch.id, st.redact(&e.to_string()));
                    break;
                }
                None => {
                    tracing::info!("live {}: provider ended the stream", ch.id);
                    break;
                }
            },
            _ = tick.tick() => {}
        }
    }
    ch.close();
    // `body` (and with it the provider connection) is dropped here.
}

fn respond(ch: &Arc<Channel>, prefix: Bytes, rx: broadcast::Receiver<Bytes>) -> Response {
    let id = ch.id.clone();
    let first = (!prefix.is_empty()).then_some(prefix);
    let s = futures_util::stream::unfold((first, rx, id), |(first, mut rx, id)| async move {
        if let Some(p) = first {
            return Some((Ok::<_, std::io::Error>(p), (None, rx, id)));
        }
        match rx.recv().await {
            Ok(b) => Some((Ok(b), (None, rx, id))),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("live {id}: viewer fell {n} chunks behind, dropping it");
                None
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    let mut r = Response::new(Body::from_stream(s));
    *r.status_mut() = ch.status;
    *r.headers_mut() = ch.headers.clone();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_to_packet_start() {
        let mut v = vec![0u8; 50];
        for _ in 0..3 {
            let mut p = vec![0xff; TS_PACKET];
            p[0] = TS_SYNC;
            v.extend(p);
        }
        // A stray 0x47 before the real packets must not fool it.
        v[10] = TS_SYNC;
        assert_eq!(ts_align(&v), 50);
        assert_eq!(ts_align(&[]), 0);
    }
}
