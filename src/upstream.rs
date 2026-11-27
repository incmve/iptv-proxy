//! HTTP client for everything that goes to the provider.

use std::time::Duration;

use reqwest::{Client, redirect};

/// Upper bound to establish a TCP/TLS connection to the provider.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A provider socket that sends nothing for this long is considered dead.
/// Only counts while we are actually waiting for data: a paused player stops
/// reading, so no read is pending and this timer does not run.
pub const READ_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// One shared client = one connection pool (the Go version's FD-leak lesson).
///
/// No total request timeout: that would cut live streams and long movies.
pub fn build(user_agent: &str) -> reqwest::Result<Client> {
    Client::builder()
        .user_agent(user_agent)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_IDLE_TIMEOUT)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        // Providers commonly 302 a stream to a load-balancer host. reqwest
        // drops sensitive headers when a redirect changes host.
        .redirect(redirect::Policy::limited(5))
        .build()
}

/// Remove every known secret from a string before it is logged.
pub fn redact(s: &str, secrets: &[&str]) -> String {
    let mut out = s.to_string();
    for sec in secrets.iter().filter(|s| s.len() >= 3) {
        out = out.replace(sec, "***");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_credentials() {
        let s = "http://p:8000/movie/provuser/provpass/1.mkv";
        assert_eq!(
            redact(s, &["provuser", "provpass"]),
            "http://p:8000/movie/***/***/1.mkv"
        );
    }

    #[test]
    fn redact_ignores_tiny_values() {
        // A 1-2 char "secret" would shred the whole log line.
        assert_eq!(redact("abc/a/b", &["a"]), "abc/a/b");
    }
}
