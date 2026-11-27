# iptv-proxy (Rust)

> **Work in progress.** This branch is a from-scratch rewrite in Rust.
> The working Go version is on `master` / `beta`.

Reverse proxy for an Xtream Codes IPTV provider. Players, Jellyfin and M3Usort
talk to the proxy with the proxy's own credentials; the proxy talks to the
provider (through Gluetun) with the real ones.

Design, decisions and milestones: [docs/DESIGN.md](docs/DESIGN.md).

## Status

All features are built and tested against a fake Xtream provider. Next: the
side-by-side test against the real provider (milestone 7).

| Feature | State |
|---|---|
| `/health`, Docker healthcheck | done |
| VOD: `/movie/…`, `/series/…` (Range, HEAD) | done |
| `player_api.php` pass-through (all fields reach the client) | done |
| Live TS, HLS (302 and 200), `/hls`, `/hlsr`, `/play` | done |
| Shared live streams (one provider connection per channel) | done |
| `get.php`, `/iptv.m3u`, `xmltv.php` (streamed, cached) | done |
| Catch-up (path + query style) | done |
| VOD throttle (optional) | done |
| Connection limit | left to the provider; slots released at once (tested) |

## Run

```bash
cp .env.example .env   # fill in M3U_URL, PROXY_USER, PROXY_PASSWORD, VPN_*
docker compose up -d
```

The compose file runs the Go proxy on 8080 and this one on 8081, side by side,
both behind Gluetun.

## Configuration

Same environment variables as the Go version; see [.env.example](.env.example).
New: `PROVIDER_USER_AGENT`, `SHARE_LIVE_STREAMS`, `XMLTV_CACHE_MINUTES`,
`CACHE_DIR`, `VOD_THROTTLE_KBPS`, `LOG_LEVEL`.

## Commands

```
iptv-proxy serve        # default
iptv-proxy healthcheck  # exit 0 if /health answers (used by the image)
iptv-proxy version
```

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

GPL-3.0-or-later, like the original project.
