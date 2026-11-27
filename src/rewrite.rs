//! Rewrites provider stream URLs and credentials in text answers
//! (player_api JSON, get.php playlists, m3u8) to the proxy's, while streaming.
//!
//! Only URLs that carry the provider credentials are touched, so logo and
//! cover URLs on the provider host keep pointing at the provider.
//!
//! PHP's `json_encode` escapes `/` as `\/`, so every pattern also exists in
//! its escaped form.

use aho_corasick::{AhoCorasick, MatchKind};
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use url::Url;

use crate::config::Config;

/// Stream path prefixes that carry `/<user>/<pass>/`.
const KINDS: &[&str] = &["live/", "movie/", "series/", "timeshift/", ""];

#[derive(Clone)]
pub struct Rewriter {
    ac: AhoCorasick,
    replacements: Vec<Vec<u8>>,
    max_len: usize,
}

impl Rewriter {
    pub fn new(cfg: &Config) -> Self {
        let pu = cfg.xtream_user.expose();
        let pp = cfg.xtream_password.expose();
        let u = cfg.user.expose();
        let p = cfg.password.expose();
        let public = cfg.public_base();

        let mut pairs: Vec<(String, String)> = Vec::new();
        for origin in provider_origins(&cfg.xtream_base_url) {
            for kind in KINDS {
                pairs.push((
                    format!("{origin}/{kind}{pu}/{pp}/"),
                    format!("{public}/{kind}{u}/{p}/"),
                ));
            }
        }
        // Any other place the provider credentials appear in a URL (a host we
        // can't proxy): swap the credentials so they never reach a client.
        pairs.push((format!("/{pu}/{pp}/"), format!("/{u}/{p}/")));
        pairs.push((
            format!("username={pu}&password={pp}"),
            format!("username={u}&password={p}"),
        ));

        // JSON-escaped variants of everything above.
        let escaped: Vec<(String, String)> = pairs
            .iter()
            .filter(|(a, _)| a.contains('/'))
            .map(|(a, b)| (a.replace('/', "\\/"), b.replace('/', "\\/")))
            .collect();
        pairs.extend(escaped);

        // Longest pattern first wins at the same position.
        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(pairs.iter().map(|(a, _)| a.as_bytes()))
            .expect("valid patterns");
        let max_len = pairs.iter().map(|(a, _)| a.len()).max().unwrap_or(1);
        Self {
            ac,
            replacements: pairs.into_iter().map(|(_, b)| b.into_bytes()).collect(),
            max_len,
        }
    }

    /// Rewrite a complete buffer.
    pub fn rewrite_all(&self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        self.ac
            .replace_all_with_bytes(input, &mut out, |m, _, dst| {
                dst.extend_from_slice(&self.replacements[m.pattern().as_usize()]);
                true
            });
        out
    }

    /// Rewrite a byte stream chunk by chunk. A match split across two chunks
    /// is still found: the last `max_len - 1` bytes are held back until the
    /// next chunk arrives.
    pub fn rewrite_stream<S, E>(
        &self,
        input: S,
    ) -> impl Stream<Item = Result<Bytes, E>> + Send + 'static
    where
        S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
        E: Send + 'static,
    {
        let rw = self.clone();
        futures_util::stream::unfold(
            (input, BytesMut::new(), false),
            move |(mut input, mut carry, done)| {
                let rw = rw.clone();
                async move {
                    if done {
                        return None;
                    }
                    loop {
                        match input.next().await {
                            Some(Ok(chunk)) => {
                                carry.extend_from_slice(&chunk);
                                let (out, rest) = rw.process(&carry, false);
                                carry = BytesMut::from(&rest[..]);
                                if !out.is_empty() {
                                    return Some((Ok(Bytes::from(out)), (input, carry, false)));
                                }
                            }
                            Some(Err(e)) => return Some((Err(e), (input, carry, true))),
                            None => {
                                let (out, _) = rw.process(&carry, true);
                                return if out.is_empty() {
                                    None
                                } else {
                                    Some((Ok(Bytes::from(out)), (input, BytesMut::new(), true)))
                                };
                            }
                        }
                    }
                }
            },
        )
    }

    /// Returns (bytes safe to emit, bytes to keep for the next round).
    fn process(&self, buf: &[u8], last: bool) -> (Vec<u8>, Vec<u8>) {
        let safe = if last {
            buf.len()
        } else {
            buf.len().saturating_sub(self.max_len - 1)
        };
        let mut out = Vec::with_capacity(buf.len());
        let mut pos = 0;
        for m in self.ac.find_iter(buf) {
            // Matches that start before `safe` are complete inside `buf`.
            if m.start() >= safe {
                break;
            }
            out.extend_from_slice(&buf[pos..m.start()]);
            out.extend_from_slice(&self.replacements[m.pattern().as_usize()]);
            pos = m.end();
        }
        let cut = pos.max(safe);
        out.extend_from_slice(&buf[pos..cut]);
        (out, buf[cut..].to_vec())
    }
}

/// `http://host:port` plus, when the port is the scheme default, the forms
/// with and without the explicit port.
fn provider_origins(base: &Url) -> Vec<String> {
    let scheme = base.scheme();
    let host = base.host_str().unwrap_or_default();
    match base.port() {
        Some(p) => vec![format!("{scheme}://{host}:{p}")],
        None => {
            let default = if scheme == "https" { 443 } else { 80 };
            vec![
                format!("{scheme}://{host}:{default}"),
                format!("{scheme}://{host}"),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn rw() -> Rewriter {
        let env: HashMap<&str, &str> = HashMap::from([
            ("USER", "test"),
            ("PASSWORD", "testpassword"),
            ("HOSTNAME", "iptvproxy"),
            ("PORT", "8080"),
            (
                "M3U_URL",
                "http://prov.example:8000/get.php?username=provuser&password=provpass",
            ),
        ]);
        Rewriter::new(&Config::from_lookup(|k| env.get(k).map(|v| v.to_string())).unwrap())
    }

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn rewrites_stream_urls_plain_and_escaped() {
        let r = rw();
        let input = r#"{"direct_source":"http:\/\/prov.example:8000\/movie\/provuser\/provpass\/12.mkv","x":"http://prov.example:8000/live/provuser/provpass/5.ts"}"#;
        assert_eq!(
            s(r.rewrite_all(input.as_bytes())),
            r#"{"direct_source":"http:\/\/iptvproxy:8080\/movie\/test\/testpassword\/12.mkv","x":"http://iptvproxy:8080/live/test/testpassword/5.ts"}"#
        );
    }

    #[test]
    fn leaves_logos_and_unrelated_text_alone() {
        let r = rw();
        let input = r#"{"stream_icon":"http:\/\/prov.example:8000\/images\/a.png","name":"provuser news","tmdb":"603"}"#;
        assert_eq!(s(r.rewrite_all(input.as_bytes())), input);
    }

    #[test]
    fn foreign_host_keeps_host_but_loses_credentials() {
        let r = rw();
        assert_eq!(
            s(r.rewrite_all(b"http://cdn.other:80/movie/provuser/provpass/1.mkv")),
            "http://cdn.other:80/movie/test/testpassword/1.mkv"
        );
        assert_eq!(
            s(r.rewrite_all(b"x.php?username=provuser&password=provpass&a=1")),
            "x.php?username=test&password=testpassword&a=1"
        );
    }

    #[tokio::test]
    async fn match_split_across_chunks_is_rewritten() {
        let r = rw();
        let text = "aaa http://prov.example:8000/live/provuser/provpass/5.ts bbb ".repeat(50);
        let expected = s(r.rewrite_all(text.as_bytes()));
        // Every chunk size from 1 to 70 bytes, so every split position occurs.
        for size in 1..70 {
            let chunks: Vec<Result<Bytes, ()>> = text
                .as_bytes()
                .chunks(size)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect();
            let out: Vec<u8> = r
                .rewrite_stream(futures_util::stream::iter(chunks))
                .map(|c| c.unwrap().to_vec())
                .concat()
                .await;
            assert_eq!(s(out), expected, "chunk size {size}");
        }
    }
}
