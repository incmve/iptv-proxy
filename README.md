# Iptv Proxy

[![Actions Status](https://github.com/pierre-emmanuelJ/iptv-proxy/workflows/CI/badge.svg)](https://github.com/pierre-emmanuelJ/iptv-proxy/actions?query=workflow%3ACI)

> Fork of [pierre-emmanuelJ/iptv-proxy](https://github.com/pierre-emmanuelJ/iptv-proxy) with the following enhancements:
> - Fixed Xtream Codes EPG not loading
> - Fixed Xtream Codes VOD data handling
> - Continue on error for malformed EXTINF entries
> - Gluetun VPN integration
> - Stream buffering for live channels (smooths network jitter)
> - Credentials moved to `.env` (no secrets in `docker-compose.yml`)

## Description

Iptv-Proxy is a reverse proxy for IPTV services. It rewrites M3U/M3U8 playlist URLs to route through the proxy, and fully proxies Xtream Codes API endpoints (live, VOD, series, EPG).

### M3U and M3U8

Converts an IPTV M3U playlist into a proxied version — all track URLs are rewritten to point at the proxy server.

### Xtream Codes client API

Full proxy for the Xtream Codes API: live streams, VOD, series, EPG, and timeshift.

---

## Quick Start

### With Docker Compose

**1. Create your `.env` file**

```bash
cp .env.example .env
# Edit .env and fill in your credentials
```

`.env` contents:
```env
OPENVPN_USER=your_vpn_user
OPENVPN_PASSWORD=your_vpn_password
M3U_URL=http://your-provider:8000/get.php?username=USER&password=PASS&type=m3u_plus&output=mpegts
PROXY_USER=myuser
PROXY_PASSWORD=mypassword
```

**2. Start**

```bash
docker-compose up -d
```

Your proxied playlist will be available at:
```
http://<HOSTNAME>:8080/iptv.m3u?username=myuser&password=mypassword
```

---

### CLI

**M3U proxy**
```bash
iptv-proxy --m3u-url http://example.com/iptv.m3u \
           --port 8080 \
           --hostname proxyexample.com \
           --user myuser \
           --password mypassword
```

**M3U + Xtream proxy**
```bash
iptv-proxy --m3u-url "http://example.com:1234/get.php?username=user&password=pass&type=m3u_plus" \
           --port 8080 \
           --hostname proxyexample.com \
           --xtream-user xtream_user \
           --xtream-password xtream_password \
           --xtream-base-url http://example.com:1234 \
           --user myuser \
           --password mypassword
```

---

## Configuration

All flags can also be set as environment variables (replace `-` with `_`, e.g. `M3U_URL`).

| Flag | Default | Description |
|---|---|---|
| `-u, --m3u-url` | _(required)_ | Remote M3U/M3U8 URL or Xtream `/get.php` URL |
| `--m3u-file-name` | `iptv.m3u` | Filename of the proxied playlist |
| `--custom-endpoint` | `""` | Optional path prefix for all endpoints |
| `--custom-id` | _(UUID)_ | Anti-collision ID embedded in track URLs |
| `--port` | `8080` | Listening port |
| `--advertised-port` | _(same as port)_ | Port used in generated URLs (useful behind a reverse proxy) |
| `--hostname` | `""` | Hostname/IP used in generated URLs |
| `--https` | `false` | Use HTTPS in generated URLs |
| `--user` | `usertest` | Proxy authentication username |
| `--password` | `passwordtest` | Proxy authentication password |
| `--xtream-user` | `""` | Original Xtream username |
| `--xtream-password` | `""` | Original Xtream password |
| `--xtream-base-url` | `""` | Original Xtream base URL |
| `--m3u-cache-expiration` | `1` | M3U cache TTL in hours |
| `--xtream-api-get` | `false` | Generate playlist via Xtream API instead of `/get.php` |
| `--buffer-enabled` | `true` | Enable stream buffering for live content |
| `--buffer-duration` | `5` | Buffer duration in seconds |
| `--buffer-max-memory` | `10` | Max memory per buffer in MB |
| `--buffer-preload` | `3` | Seconds to pre-buffer before playback starts |

---

## Stream Buffering

Live streams are automatically buffered to absorb network jitter from the upstream provider.

**How it works:**
- A ring buffer per stream accumulates data ahead of the client
- Multiple clients share a single upstream connection for the same stream
- Buffering is skipped automatically for HLS segments (`.m3u8`, `.ts`) and VOD/series content
- Stale readers (inactive for 2+ minutes) are cleaned up automatically
- A buffer with no readers is released after a 30-second grace period

**Disable buffering:**
```bash
--buffer-enabled=false
# or
BUFFER_ENABLED=false
```

**Monitor active buffers** (requires authentication):
```
GET http://<hostname>:<port>/buffer-stats?username=<user>&password=<pass>
```

Example response:
```json
{
  "total_buffers": 2,
  "buffer_time": 5,
  "buffers": {
    "http://provider/live/...": {
      "capacity": 40,
      "size": 38,
      "total_bytes": 1245184,
      "readers": 1,
      "buffer_time": 5
    }
  }
}
```

---

## M3U Example

Original playlist:
```m3u
#EXTM3U
#EXTINF:-1 tvg-ID="examplechanel1.com" tvg-name="chanel1" tvg-logo="http://ch.xyz/logo1.png" group-title="USA HD",CHANEL1-HD
http://iptvexample.net:1234/12/test/1
```

Proxied playlist:
```m3u
#EXTM3U
#EXTINF:-1 tvg-ID="examplechanel1.com" tvg-name="chanel1" tvg-logo="http://ch.xyz/logo1.png" group-title="USA HD",CHANEL1-HD
http://proxyserver.com:8080/<id>/myuser/mypassword/0/1?username=myuser&password=mypassword
```

---

## Xtream Codes Example

Original credentials:
```
user:     xtream_user
password: xtream_password
base-url: http://example.com:1234
```

Proxy credentials (what your client uses):
```
user:     myuser
password: mypassword
base-url: http://proxyexample.com:8080
```

Get the proxied playlist:
```
http://proxyexample.com:8080/get.php?username=myuser&password=mypassword&type=m3u_plus&output=ts
```

---

## TODO

- Replace username/password-in-URL auth with token-based auth
- Replace in-memory cache map with Redis/etcd for multi-instance deployments

---

## Powered by

- [cobra](https://github.com/spf13/cobra)
- [go.xtream-codes](https://github.com/tellytv/go.xtream-codes)
- [gin](https://github.com/gin-gonic/gin)
- [gluetun](https://github.com/qdm12/gluetun)
