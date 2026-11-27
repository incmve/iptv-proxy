//! Which headers may cross the proxy.
//!
//! The Go version copied every header in both directions. This version uses a
//! blacklist (same idea as tuliprox's `header_policy.rs`): everything passes
//! except hop-by-hop headers and headers that carry credentials or cookies.
//! Range, If-Range, User-Agent, Accept and so on keep working.

use axum::http::{HeaderMap, HeaderName, header};

/// Never forwarded, in either direction.
const BLOCKED: &[&str] = &[
    "authorization",
    "connection",
    "cookie",
    "cookie2",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "set-cookie",
    "te",
    "trailer",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

/// Request headers that would reveal the client or the LAN to the provider.
const BLOCKED_UPSTREAM_PREFIXES: &[&str] = &[
    "x-forwarded-",
    "x-real-ip",
    "forwarded",
    "cf-",
    "x-client-ip",
];

fn is_blocked(name: &HeaderName) -> bool {
    BLOCKED.contains(&name.as_str())
}

/// Client request headers -> provider request headers.
pub fn upstream_request(client: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in client {
        let n = k.as_str();
        if is_blocked(k) || BLOCKED_UPSTREAM_PREFIXES.iter().any(|p| n.starts_with(p)) {
            continue;
        }
        // Let reqwest negotiate compression itself; a forwarded Accept-Encoding
        // could make us relay a compressed body with the wrong framing.
        if k == header::ACCEPT_ENCODING {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

/// Provider response headers -> client response headers.
pub fn client_response(upstream: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in upstream {
        if is_blocked(k) {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn map(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(HeaderName::from_static(k), HeaderValue::from_static(v));
        }
        m
    }

    #[test]
    fn upstream_keeps_range_and_user_agent_drops_secrets() {
        let out = upstream_request(&map(&[
            ("range", "bytes=100-"),
            ("user-agent", "TiviMate/5"),
            ("cookie", "a=b"),
            ("authorization", "Basic x"),
            ("host", "192.168.0.58:8080"),
            ("x-forwarded-for", "192.168.0.10"),
            ("accept-encoding", "gzip"),
        ]));
        assert_eq!(out.get("range").unwrap(), "bytes=100-");
        assert_eq!(out.get("user-agent").unwrap(), "TiviMate/5");
        for k in [
            "cookie",
            "authorization",
            "host",
            "x-forwarded-for",
            "accept-encoding",
        ] {
            assert!(out.get(k).is_none(), "{k} leaked upstream");
        }
    }

    #[test]
    fn response_keeps_content_headers_drops_hop_by_hop() {
        let out = client_response(&map(&[
            ("content-range", "bytes 100-199/1000"),
            ("content-length", "100"),
            ("accept-ranges", "bytes"),
            ("set-cookie", "s=1"),
            ("transfer-encoding", "chunked"),
            ("connection", "keep-alive"),
        ]));
        assert_eq!(out.get("content-range").unwrap(), "bytes 100-199/1000");
        assert_eq!(out.get("content-length").unwrap(), "100");
        assert!(out.get("set-cookie").is_none());
        assert!(out.get("transfer-encoding").is_none());
        assert!(out.get("connection").is_none());
    }
}
