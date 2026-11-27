# iptv-proxy (Rust) — design

A from-scratch Rust rewrite of the Go `iptv-proxy` fork (`master`/`beta`).
Same job, same URLs, fewer bugs. [tuliprox](https://github.com/euzu/tuliprox)
(MIT) is used as a reference for proven approaches; no Go code is translated.

## Clients

| Client | Uses |
|---|---|
| M3Usort | `player_api.php` (catalog, `get_vod_info`, `get_series_info`) |
| Jellyfin | `.strm` files → `/movie/…` and `/series/…` (needs Range for seeking) |
| TiviMate and other apps | Xtream login, live, EPG, catch-up, VOD, series |

## Compatibility contract (must not break)

- Same environment variables as the Go version (`M3U_URL`, `PORT`, `HOSTNAME`,
  `ADVERTISED_PORT`, `HTTPS`, `USER`, `PASSWORD`, `XTREAM_*`,
  `M3U_CACHE_EXPIRATION`, `M3U_FILE_NAME`). Go-only variables are ignored with
  a warning.
- Same stream URLs, with the proxy credentials in the path, so existing `.strm`
  files and app logins keep working:
  `/live/u/p/{id}`, `/u/p/{id}`, `/movie/u/p/{id}`, `/series/u/p/{id}`,
  `/timeshift/u/p/{duration}/{start}/{id}`, `/play/{token}/{type}`,
  `/hls/{token}/{chunk}`, `/hlsr/{token}/u/p/{channel}/{hash}/{chunk}`.
- Same API endpoints: `/player_api.php` (GET+POST), `/get.php` (GET+POST),
  `/xmltv.php`, `/{M3U_FILE_NAME}`, `/health`.

## Decisions

1. **`player_api.php` is passed through as-is.** The Go version decoded answers
   into fixed structs and re-encoded them, which dropped every field it didn't
   know (TMDB IDs for series and lists). Only the login answer (no `action`) is
   rewritten so `server_info`/`user_info` point at the proxy. Provider URLs and
   credentials inside any text answer (JSON `direct_source`, m3u, m3u8) are
   rewritten to the proxy's.
2. **No global lock.** The Go version serialised every provider API call behind
   one mutex: a big `get_vod_streams` or EPG download froze all other clients.
3. **Header blacklist, not "copy everything"** (as tuliprox `header_policy.rs`).
   Never forwarded: hop-by-hop headers, `authorization`, `cookie`,
   `set-cookie`, `host`, proxy headers. Never sent to the provider:
   `x-forwarded-*`, `x-real-ip`, `forwarded`. Everything else passes (Range,
   User-Agent, Content-Range, …).
4. **Retry only while opening a stream** (tuliprox defaults: 3 attempts, 250 ms,
   ×1.5, honour `Retry-After`, max 5 s). Once bytes flow, a drop ends the
   response and the player reconnects.
5. **Timeouts:** 10 s connect, 30 s read-idle, no total timeout (it would cut
   live streams and movies).
6. **Provider User-Agent:** fixed (`PROVIDER_USER_AGENT`, default `iptv-proxy`)
   for the proxy's own API calls; streams forward the player's User-Agent.
7. **Unknown `player_api` actions are passed through**, not answered with the
   login (Go bug).
8. **HLS:** a `.m3u8` answered with 200 is relayed with its body (Go only
   handled 302 and sent an empty body otherwise). Redirect hosts per channel
   are kept with an expiry instead of a map that grows forever.
9. **EPG and get.php are streamed**, never loaded whole into memory; cached on
   disk for `M3U_CACHE_EXPIRATION` hours.
10. **Catch-up: both URL styles** — path (`/timeshift/…`) and query
    (`/timeshift.php`, `/streaming/timeshift.php`, GET+POST), as tuliprox does.
11. **No playlist download at startup.** The Go version parsed the whole
    `m3u_plus` list (5 retries) on every start, even in Xtream mode.
12. **Logs never contain credentials** (provider user/password, proxy password).
    Values shorter than 3 characters are not redacted.
13. **Health:** `/health` never touches the provider. The image has no shell;
    `iptv-proxy healthcheck` does the Docker check.

## Connection limit (provider allows 2)

**The proxy does not decide who may watch.** The provider enforces its own
limit (it refuses or drops a connection); a second set of rules in the proxy
could only disagree with it. The proxy's job is to never waste a slot:

- When a player disconnects, the provider connection closes immediately
  (tested in `tests/disconnect.rs`: two players, each hangs up, the upstream
  count goes 2 → 1 → 0 within milliseconds).
- No parallel or speculative upstream requests; open-retries are sequential
  and only on 5xx/connect errors, so a provider "limit reached" answer (4xx)
  is passed straight to the player.
- The provider's answer (status and body) reaches the player unchanged, so the
  player shows the provider's own message.
- `player_api.php`, `xmltv.php` and `get.php` are not streams. To verify on the
  real provider: `active_cons` in the login answer while M3Usort runs its
  catalog job should stay 0.

## Dropped from the Go version

- M3U-file mode (per-track routes) — only Xtream providers are used.
- `CUSTOM_ENDPOINT`, `CUSTOM_ID`, `XTREAM_API_GET`.
- Live buffering (`BUFFER_*`) — off in production; revisit only if live
  streams stutter.

## Out of scope (tuliprox has it, we don't need it)

Multiple accounts/failover, users and roles, web UI, mapping DSL, DVR, plugins,
shared HLS segment cache, HDHomeRun.

## Milestones

- [x] **1. Skeleton + VOD:** config, `/health` + `healthcheck`, `/movie` and
      `/series` (GET+HEAD, Range), header policy, open-retry, redaction,
      Docker image, CI.
- [ ] **2. `player_api.php`:** pass-through for all actions (GET+POST), login
      rewrite, URL/credential rewrite in answers.
- [ ] **3. Live + HLS:** `/live`, `/{u}/{p}/{id}`, `/play`, `/hls`, `/hlsr`,
      m3u8 rewrite, the 200 fix.
- [ ] **4. EPG + playlists:** streamed `xmltv.php`, `get.php`,
      `/{M3U_FILE_NAME}`, disk cache.
- [ ] **5. Catch-up:** path and query styles.
- [x] ~~**6. Connection limit**~~ — dropped: the provider enforces its limit.
      The proxy only guarantees it releases slots at once (tested).
- [ ] **7. Side-by-side test** on port 8081 behind Gluetun: compare JSON answers
      with the Go proxy, add as a second playlist in TiviMate, then switch.
- [ ] **8. Shared live streams** (optional): two players on the same live
      channel share one provider connection. Saves a slot instead of deciding
      who gets one.
- [ ] **9. VOD throttle** (optional, off by default).
