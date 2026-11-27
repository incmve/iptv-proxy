//! Configuration from environment variables.
//!
//! The variable names are the same as the Go version, so the Rust proxy is a
//! drop-in replacement in an existing docker-compose file.

use std::env;
use std::fmt;

use url::Url;

/// A secret that never shows up in `Debug` output or logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Port the proxy listens on (`PORT`, default 8080).
    pub port: u16,
    /// Hostname or IP written into rewritten URLs (`HOSTNAME`).
    pub hostname: String,
    /// Port written into rewritten URLs (`ADVERTISED_PORT`, default = `PORT`).
    pub advertised_port: u16,
    /// Write `https://` into rewritten URLs (`HTTPS`).
    pub https: bool,

    /// Credentials clients use to talk to this proxy (`USER` / `PASSWORD`).
    pub user: Secret,
    pub password: Secret,

    /// Provider (Xtream Codes) base URL, e.g. `http://provider:8000`.
    pub xtream_base_url: Url,
    pub xtream_user: Secret,
    pub xtream_password: Secret,

    /// User-Agent for the proxy's own provider API calls (`PROVIDER_USER_AGENT`).
    /// Stream requests forward the player's own User-Agent instead.
    pub provider_user_agent: String,

    /// Hours to cache `get.php` playlists (`M3U_CACHE_EXPIRATION`, default 1).
    pub m3u_cache_hours: u64,
    /// File name of the proxied playlist (`M3U_FILE_NAME`, default `iptv.m3u`).
    pub m3u_file_name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{name} has an invalid value: {reason}")]
    Invalid { name: &'static str, reason: String },
}

/// Variables from the Go version that this version deliberately ignores.
const IGNORED: &[(&str, &str)] = &[
    ("GIN_MODE", "Go-only (web framework)"),
    (
        "BUFFER_ENABLED",
        "live buffering is not ported; see docs/DESIGN.md",
    ),
    ("BUFFER_DURATION", "live buffering is not ported"),
    ("BUFFER_MAX_MEMORY", "live buffering is not ported"),
    ("BUFFER_PRELOAD", "live buffering is not ported"),
    ("CUSTOM_ENDPOINT", "dropped; see docs/DESIGN.md"),
    ("CUSTOM_ID", "M3U-file mode is dropped"),
    (
        "XTREAM_API_GET",
        "dropped; get.php is always passed through",
    ),
];

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| env::var(k).ok())
    }

    /// Build the config from any key/value source (used by tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        for (name, why) in IGNORED {
            if get(name).is_some() {
                tracing::warn!("{name} is set but ignored: {why}");
            }
        }

        let port = parse_num(get("PORT"), "PORT", 8080u16)?;
        let advertised_port = match parse_num(get("ADVERTISED_PORT"), "ADVERTISED_PORT", 0u16)? {
            0 => port,
            p => p,
        };

        let user = Secret::new(get("USER").ok_or(ConfigError::Missing("USER"))?);
        let password = Secret::new(get("PASSWORD").ok_or(ConfigError::Missing("PASSWORD"))?);

        // Provider: explicit XTREAM_* variables win; otherwise take them from a
        // get.php M3U_URL, the same way the Go version does.
        let (xtream_base_url, xtream_user, xtream_password) = match (
            get("XTREAM_BASE_URL"),
            get("XTREAM_USER"),
            get("XTREAM_PASSWORD"),
        ) {
            (Some(base), Some(u), Some(p)) => (parse_base_url(&base, "XTREAM_BASE_URL")?, u, p),
            (None, None, None) => {
                let m3u = get("M3U_URL").ok_or(ConfigError::Missing(
                    "M3U_URL (or XTREAM_BASE_URL, XTREAM_USER and XTREAM_PASSWORD)",
                ))?;
                from_get_php(&m3u)?
            }
            _ => {
                return Err(ConfigError::Invalid {
                    name: "XTREAM_*",
                    reason:
                        "set all three of XTREAM_BASE_URL, XTREAM_USER, XTREAM_PASSWORD, or none"
                            .into(),
                });
            }
        };

        let user_ = Secret::new(xtream_user);
        let pass_ = Secret::new(xtream_password);

        Ok(Self {
            port,
            hostname: get("HOSTNAME").unwrap_or_else(|| "localhost".into()),
            advertised_port,
            https: parse_bool(get("HTTPS")),
            user,
            password,
            xtream_base_url,
            xtream_user: user_,
            xtream_password: pass_,
            provider_user_agent: get("PROVIDER_USER_AGENT").unwrap_or_else(|| "iptv-proxy".into()),
            m3u_cache_hours: parse_num(get("M3U_CACHE_EXPIRATION"), "M3U_CACHE_EXPIRATION", 1u64)?,
            m3u_file_name: get("M3U_FILE_NAME").unwrap_or_else(|| "iptv.m3u".into()),
        })
    }

    /// `http(s)://hostname:port` as clients should see this proxy.
    pub fn public_base(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.hostname, self.advertised_port)
    }
}

fn parse_num<T: std::str::FromStr>(
    v: Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    match v {
        None => Ok(default),
        Some(s) => s.parse().map_err(|_| ConfigError::Invalid {
            name,
            reason: format!("{s:?} is not a valid number"),
        }),
    }
}

fn parse_bool(v: Option<String>) -> bool {
    matches!(
        v.as_deref().map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn parse_base_url(s: &str, name: &'static str) -> Result<Url, ConfigError> {
    let u = Url::parse(s).map_err(|e| ConfigError::Invalid {
        name,
        reason: e.to_string(),
    })?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(ConfigError::Invalid {
            name,
            reason: "must be an http(s) URL with a host".into(),
        });
    }
    // Keep scheme://host[:port] only.
    let mut base = u.clone();
    base.set_path("");
    base.set_query(None);
    base.set_fragment(None);
    Ok(base)
}

fn from_get_php(m3u: &str) -> Result<(Url, String, String), ConfigError> {
    let invalid = |reason: &str| ConfigError::Invalid {
        name: "M3U_URL",
        reason: reason.into(),
    };
    let u = Url::parse(m3u).map_err(|e| invalid(&e.to_string()))?;
    if !u.path().ends_with("/get.php") {
        return Err(invalid(
            "only Xtream get.php URLs are supported (M3U-file mode was dropped); \
             or set XTREAM_BASE_URL, XTREAM_USER and XTREAM_PASSWORD",
        ));
    }
    let q = |k: &str| {
        u.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.into_owned())
            .filter(|v| !v.is_empty())
    };
    let user = q("username").ok_or_else(|| invalid("get.php URL has no username"))?;
    let pass = q("password").ok_or_else(|| invalid("get.php URL has no password"))?;
    Ok((parse_base_url(m3u, "M3U_URL")?, user, pass))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| m.get(k).cloned())
    }

    const BASE: &[(&str, &str)] = &[
        ("USER", "test"),
        ("PASSWORD", "testpassword"),
        (
            "M3U_URL",
            "http://prov.example:8000/get.php?username=pu&password=pp&type=m3u_plus&output=mpegts",
        ),
    ];

    #[test]
    fn derives_xtream_from_get_php() {
        let c = cfg(BASE).unwrap();
        assert_eq!(c.xtream_base_url.as_str(), "http://prov.example:8000/");
        assert_eq!(c.xtream_user.expose(), "pu");
        assert_eq!(c.xtream_password.expose(), "pp");
        assert_eq!(c.port, 8080);
        assert_eq!(c.advertised_port, 8080);
        assert_eq!(c.provider_user_agent, "iptv-proxy");
    }

    #[test]
    fn advertised_port_defaults_to_port() {
        let mut v = BASE.to_vec();
        v.push(("PORT", "8081"));
        assert_eq!(cfg(&v).unwrap().advertised_port, 8081);
        v.push(("ADVERTISED_PORT", "443"));
        assert_eq!(cfg(&v).unwrap().advertised_port, 443);
    }

    #[test]
    fn rejects_plain_m3u_and_partial_xtream() {
        let e = cfg(&[
            ("USER", "a"),
            ("PASSWORD", "b"),
            ("M3U_URL", "http://x/list.m3u"),
        ])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError::Invalid {
                name: "M3U_URL",
                ..
            }
        ));
        let e = cfg(&[("USER", "a"), ("PASSWORD", "b"), ("XTREAM_USER", "u")]).unwrap_err();
        assert!(matches!(
            e,
            ConfigError::Invalid {
                name: "XTREAM_*",
                ..
            }
        ));
    }

    #[test]
    fn requires_proxy_credentials() {
        assert_eq!(cfg(&BASE[1..]).unwrap_err(), ConfigError::Missing("USER"));
    }

    #[test]
    fn secrets_are_not_in_debug_output() {
        let s = format!("{:?}", cfg(BASE).unwrap());
        assert!(!s.contains("testpassword") && !s.contains("\"pp\""));
    }
}
