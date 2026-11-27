//! The provider allows only a few connections, and the provider (not the
//! proxy) enforces that limit. So the proxy must never hold a provider
//! connection longer than the player does: when the player disconnects, the
//! upstream connection must close right away.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, body::Body, extract::State, response::IntoResponse, routing::get};
use bytes::Bytes;
use iptv_proxy::{AppState, Config, router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Live upstream streams currently open at the fake provider.
#[derive(Default)]
struct Open(AtomicUsize);

struct Guard(Arc<Open>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An endless "live" stream that counts itself as open until hyper drops it.
async fn endless(State(open): State<Arc<Open>>) -> impl IntoResponse {
    open.0.fetch_add(1, Ordering::SeqCst);
    let guard = Guard(open.clone());
    let s = futures_util::stream::unfold(guard, |g| async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some((
            Ok::<_, std::io::Error>(Bytes::from_static(&[0x47; 188 * 7])),
            g,
        ))
    });
    Body::from_stream(s)
}

async fn spawn(app: Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    a
}

async fn wait_until(open: &Open, want: usize) -> bool {
    for _ in 0..100 {
        if open.0.load(Ordering::SeqCst) == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn player_disconnect_closes_provider_connection_immediately() {
    let open = Arc::new(Open::default());
    let prov = Router::new()
        .route("/movie/{u}/{p}/{id}", get(endless))
        .with_state(open.clone());
    let prov_addr = spawn(prov).await;

    let env: HashMap<&str, String> = HashMap::from([
        ("USER", "test".into()),
        ("PASSWORD", "testpassword".into()),
        (
            "M3U_URL",
            format!("http://{prov_addr}/get.php?username=pu&password=pp"),
        ),
    ]);
    let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
    let proxy_addr = spawn(router(Arc::new(AppState::new(cfg).unwrap()))).await;

    // Two players, like the provider limit of 2.
    let mut players = Vec::new();
    for _ in 0..2 {
        let mut s = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        s.write_all(b"GET /movie/test/testpassword/1.ts HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).await.unwrap();
        assert!(n > 0, "no data from proxy");
        players.push(s);
    }
    assert!(wait_until(&open, 2).await, "expected 2 open upstreams");

    // One player hangs up (zapping / closing the app).
    drop(players.pop());
    assert!(
        wait_until(&open, 1).await,
        "upstream not released after player disconnect"
    );

    // The other one too.
    drop(players.pop());
    assert!(
        wait_until(&open, 0).await,
        "upstream not released after player disconnect"
    );
}
