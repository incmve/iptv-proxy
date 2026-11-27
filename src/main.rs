use std::{process::ExitCode, sync::Arc, time::Duration};

use iptv_proxy::{AppState, Config, router};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: iptv-proxy [serve|healthcheck|version]";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => rt.block_on(serve()),
        Some("healthcheck") => rt.block_on(healthcheck()),
        Some("version" | "--version" | "-V") => {
            println!("iptv-proxy {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command {other:?}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> ExitCode {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("configuration error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let port = cfg.port;
    tracing::info!(
        "iptv-proxy {} starting; provider {}, advertised as {}",
        env!("CARGO_PKG_VERSION"),
        cfg.xtream_base_url,
        cfg.public_base()
    );

    let state = match AppState::new(cfg) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::error!("cannot build HTTP client: {e}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("cannot listen on port {port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!("listening on :{port}");

    let shutdown = async {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        let _ = ctrl_c.await;
        tracing::info!("shutting down");
    };

    match axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("server error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// For the Docker HEALTHCHECK: the runtime image has no shell, wget or curl.
/// Plain HTTP/1.0 over TCP to keep it dependency-free.
async fn healthcheck() -> ExitCode {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(8080);
    let check = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        s.write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(
            buf.starts_with(b"HTTP/1.0 200") || buf.starts_with(b"HTTP/1.1 200"),
        )
    };
    match tokio::time::timeout(Duration::from_secs(3), check).await {
        Ok(Ok(true)) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
