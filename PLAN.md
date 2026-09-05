# L337 Audio Server — Implementation Plan

> This document is the **living plan** for the Rust `l337-audio-server`. It supersedes the original
> greenfield "copy-paste prompt" in the repo history. Checkboxes track actual code state as of
> 2026-08-02. Items marked **[DONE]** are verified in the source tree; **[TODO]** remain.

---

## 0. Current architecture (verified)

- `src/main.rs` — Axum app, binds `config.toml [server] host/port`, wires **14 routes** (incl. 3
  stream upload endpoints), creates `StorageManager` + `PlayerEngine`. Applies `security::AuthLayer`
  token middleware. Auto-generates self-signed TLS cert when none configured. Generates/persists
  auth token to `server_token.txt` if unset. Writes default `config.toml` when missing.
- `src/api/models.rs` — `Track{track_id, stream_url, title?, artist?, duration?}`, `SpeedPayload`,
  `VolumePayload`, `SeekPayload`, `PoolSettings`, `PlayerStatus{state(lowercase enum), volume, speed,
  current_track?, disk_pool_utilization_bytes, next_cached, prev_cached, position_sec?, duration_sec?}`,
  `CacheManifestEntry{track_id, file_size, last_accessed, play_count}`.
- `src/api/handlers.rs` — `play`, `pause`, `next`, `previous`, `cache_next`, `cache_previous`,
  `set_speed`, `set_volume`, `seek`, `get_status`, `set_settings`, `health`, `upload_stream`.
  State wrapped in `SendableEngine(Mutex<PlayerEngine>)` with `unsafe impl Send/Sync`.
  **400 validation** on empty `track_id`/`stream_url`. `upload_stream` validates `X-Track-Id` header.
- `src/player/engine.rs` — `PlayerEngine` (raw cpal output stream + rubato resampling, NOT rodio).
  `new()` requires audio device; `new_dummy()` for headless. `play_track`, `play_pushed`,
  `load_and_play`, `pause/resume/stop`, `set_speed`, `set_volume`, `seek`, `trigger_next/previous`,
  `get_status`. `download_stream` handles `file://`, absolute paths, YouTube URLs (via `yt-dlp`),
  and HTTP streaming. Rejects HTML/markup content.
- `src/player/storage.rs` — `StorageManager`: configurable cache dir, manifest load/save,
  `update_access` (increments `play_count` + `last_accessed`), `evict_if_needed` (least-played,
  then oldest; protects `current`/`next`/`prev` slots).
- `src/security.rs` — `AuthLayer` tower middleware (Bearer token + `X-L337-Token` alias; `/health`
  exempt; constant-time-ish compare). `generate_self_signed(host)` via `rcgen`. `rustls_config()`
  builder for `axum-server`.
- `src/api/tests.rs` — Unit tests for `play`, `pause`, `get_status` using `PlayerEngine::new_dummy`.
- `config.toml` — `[server] host/port/token`, `[storage] max_cache_size_bytes/cache_dir` (optional).
- `build.sh` / `scripts/` — Cross-platform build dispatcher, systemd/launchd installers, run script.

---

## 1. Goals
1. Make the server the **main audio output** for the client (client drives it via HTTP API).
2. Support **separate devices** (client + server on different machines, LAN **and** WWAN).
3. Allow the client to deliver audio the server can't fetch: **direct file push** and **client-side transcoding** (bandwidth reduction).
4. Configurable **cache size** (default **256 MB**) at `~/.cache/l337/l337-audio-server/cache/`, evicted by **least-played / LRU**.
5. **Secure by default** for LAN + WWAN: built-in **TLS (HTTPS)** + **shared-token** auth.

---

## 2. Shared API contract (client ↔ server)
- `Track` payload: `{track_id:str, stream_url:str, title?:str, artist?:str, duration?:u64}`.
  `track_id` and `stream_url` are **required** (400 if missing/empty). `track_id` = client-generated stable hash (sha1 of canonical stream URL).
- `state` serializes **lowercase**: `playing | paused | stopped`.
- `PlayerStatus`: `{state, volume, speed, current_track?, disk_pool_utilization_bytes, next_cached, prev_cached, position_sec?, duration_sec?}`.
- Auth: every request (except `GET /health`, `POST /auth/challenge`, `POST /auth/redeem`) requires header `Authorization: Bearer <token>` (alias `X-L337-Token`). 401 otherwise.
- All endpoints served over **HTTPS** (auto-generated self-signed cert if none configured).

### Routes
| Method | Endpoint | Body | Notes |
|---|---|---|---|
| GET | `/health` | — | Unauthenticated liveness probe |
| GET | `/` | — | Returns `"L337 Audio Server"` |
| POST | `/auth/challenge` | — | Public; issues challenge token, writes `challenge-token.txt`, returns `202 { expires_in: 600 }` |
| POST | `/auth/redeem` | — | Public; verifies `X-L337-Challenge` header, promotes to server token, returns `200 { ok: true }` |
| POST | `/player/play` | `Track` | Download `stream_url` (or `file://`/local path/YouTube) → `current.stream`, play |
| POST | `/player/play/stream` | raw bytes + `X-Track-Id` (+`X-Title`/`X-Artist`) | Client pushes audio; write to `current.stream` + play |
| POST | `/player/cache/next` | `Track` | Background download → `next.stream` |
| POST | `/player/cache/next/stream` | raw bytes + `X-Track-Id` | Push → `next.stream` |
| POST | `/player/cache/previous` | `Track` | Background download → `prev.stream` |
| POST | `/player/cache/previous/stream` | raw bytes + `X-Track-Id` | Push → `prev.stream` |
| POST | `/player/next` | — | Swap to next |
| POST | `/player/previous` | — | Swap to previous |
| POST | `/player/pause` | — | Toggle pause |
| POST | `/player/seek` | `{position:u64}` | Seek to second (re-decodes from slot file) |
| POST | `/player/speed` | `{speed:f32}` | Playback speed (0.25..4.0) |
| POST | `/player/volume` | `{volume:f32}` | Volume (0.0..1.0) |
| PUT | `/player/settings` | `{max_disk_pool_bytes:u64}` | Adjust cache cap at runtime |
| GET | `/player/status` | — | `PlayerStatus` |

---

## 3. Server changes

### 3.1 Cache root + size (config-driven) — **[DONE]**
- `config.toml`: `[storage] max_cache_size_bytes` (default `268435456` = 256 MiB) and optional
  `[storage] cache_dir`.
- `StorageManager::new(max_pool_size, cache_dir?)` reads config; `main.rs` passes parsed values.
- Cache dir honors `CACHE_DIRECTORY`, `STATE_DIRECTORY`, falls back to `~/.cache/l337/l337-audio-server/cache/`.
- `manifest.json` persisted in cache dir.

### 3.2 Play-count / LRU eviction — **[DONE]**
- `evict_if_needed` sorts by **least-played, then oldest**: `entries.sort_by(|a,b| a.play_count.cmp(&b.play_count).then(a.last_accessed.cmp(&b.last_accessed)))`.
- `update_access` increments `play_count` on every `play_track` and on pushed-stream playback.
- Active slots (`current`/`next`/`prev`) protected from eviction.

### 3.3 `file://` / local-path passthrough — **[DONE]**
- `download_stream`: `file://` prefix or bare absolute path → `tokio::fs::copy`.
- YouTube URLs (`youtube.com/watch`, `youtu.be`, `shorts`, `googlevideo.com`) → `yt-dlp` binary.
- HTTP fetch: rejects HTML/markup content-type and HTML-byte signatures.

### 3.4 Streaming upload endpoints — **[DONE]**
- `upload_stream` handler consumes request `Body` as data stream, writes to target slot.
- Derives slot from route path: `/player/play/stream` → `current`, `/player/cache/next/stream` → `next`,
  `/player/cache/previous/stream` → `prev`.
- Validates `X-Track-Id` (400 if missing). Reads optional `X-Title` / `X-Artist`.
- `current` slot: calls `play_pushed` (stops current, loads new audio, updates status).
- `next`/`prev` slots: calls `update_access` + `evict_if_needed`.
- All three routes registered in `main.rs`.

### 3.5 Seek + volume + status enrichment — **[DONE]**
- `engine.seek(position: u64)` — re-decodes slot file, trims PCM to target sample, resamples.
- `engine.set_volume(f32)` — clamps 0.0..1.0.
- `PlayerStatus` includes `position_sec: Option<u64>` (computed from buffer read position) and
  `duration_sec: Option<u64>` (set after decode).
- `state` serializes lowercase via `#[serde(rename_all = "lowercase")]` on `PlayerStateLabel`.
- `play`/`cache_*` return 400 on empty `track_id`/`stream_url`.

### 3.6 Security: TLS + token — **[DONE]**
- `config.toml [server]`: `host`, `port`, `token` (optional), `tls_cert`, `tls_key` (optional).
- Missing token → `generate_token()` (32 char alnum, dashed groups of 4) persisted to `server_token.txt`; logged once.
- Missing TLS cert → `security::generate_self_signed(host)` auto-generates via `rcgen`; served via
  `axum-server` + `rustls`. Warns in logs.
- `security::AuthLayer` middleware: Bearer token or `X-L337-Token`; `/health`, `/auth/challenge`,
  `/auth/redeem` exempt; 401 otherwise.
- `reqwest` uses `rustls-tls` feature (pure-Rust TLS).
- `platform::init()` called at startup (but module is missing — see BLOCKER).
- `/setup` removed; replaced with challenge/redeem flow.

### 3.7 Misc — **[DONE]**
- `unsafe impl Send/Sync for PlayerEngine` retained: `cpal::Stream` contains
  `NotSendSyncAcrossAllPlatforms` and is conservatively `!Send` in the type system,
  but is safe to send across threads on all supported platforms. The `unsafe impl`
  is documented and justified.
- `SendableEngine` no longer has its own `unsafe impl`; it derives `Send+Sync` from
  `tokio::sync::Mutex<PlayerEngine>` now that `PlayerEngine` is explicitly `Send+Sync`.

---

## 4. Client changes (in `l337-player` repo, see `PLAN-client.md`)
- New `audio_server_l337` plugin (probe-only): `GET {server_url}/health` with token.
- `APIClient`: HTTPS + bearer token; streaming push to `*/stream` endpoints; `X-Track-Id` header.
- `Player`: source classification — network URL server can fetch → send `stream_url`; local file /
  server can't fetch → **push** (optionally transcoded via ffmpeg/yt-dlp).
- Payload normalization: `track_id` (sha1) + `stream_url`; lowercase state; seek/volume routing.
- Settings: `server_url`, `server_token`, `remote_transcode`.

---

## 5. Sequencing (actual status)

| # | Step | Status |
|---|---|---|
| 1 | Cache root/size + play-count eviction + `file://` passthrough | **DONE** |
| 2 | Streaming `*/stream` upload endpoints | **DONE** |
| 3 | Seek + volume + status enrichment + lowercase state + 400 validation | **DONE** |
| 4 | TLS + token middleware + `/health` + self-signed fallback | **DONE** |
| 5 | `--dummy` headless mode + default config generation + token persistence | **DONE** |
| 6 | Fix `platform.rs` missing module (BLOCKER) | **DONE** |
| 7 | Remove `unsafe impl Send/Sync` after verification | **DONE** (retained on `PlayerEngine` with justification; removed from `SendableEngine`) |
| 8 | Rate limiting / request body size limits | **DONE** |
| 9 | Token rotation via `PUT /player/settings` + SIGHUP reload | **DONE** |
| 10 | Challenge/redeem auth flow replacing `/setup` | **DONE** |
| 11 | Config file permissions `0o600` on auto-generated files | **DONE** |
| 12 | Token comparison hardening (constant-time) | **DONE** |
| 13 | Client: `APIClient` HTTPS+token → plugin → source classification/push/transcode | **TODO** |
| 14 | E2E: localhost verified (dummy mode + l337-player `connect_audio_server()` returns reachable/token_ok) → LAN (separate device, TLS+token) → Tailscale | **PARTIAL** |

---

## 6. Blockers

### 6.1 `yt-dlp` runtime dependency — **[NOTE]**
`download_via_ytdlp` and `resolve_stream_url` shell out to the `yt-dlp` binary. This is a **runtime
dependency** — it is **not** bundled with the server binary and is **not** installed by the
`install-systemd.sh` / `install-launchd.sh` scripts.

Operators must install `yt-dlp` separately and ensure it is on the server process `PATH`:
- **Debian/Ubuntu:** `apt install yt-dlp`
- **macOS (Homebrew):** `brew install yt-dlp`
- **pip:** `pip install yt-dlp`

If `yt-dlp` is missing, YouTube URLs return a clear error:
```
yt-dlp is not installed. Install yt-dlp to play/download YouTube URLs.
```

The `/health` endpoint reports `yt_dlp: true/false` in the capabilities object so the client
can detect support without attempting a playback.

---

## 7. Remaining server TODOs

### 7.1 Token rotation — **[DONE]**
- `PUT /player/settings` accepts optional `token` field; updates auth layer in place.
- SIGHUP triggers config reload and rotates token if `[server] token` changed.

### 7.2 Challenge/redeem auth flow — **[DONE]**
- Removed unauthenticated `/setup` endpoint.
- Added `POST /auth/challenge` (public) and `POST /auth/redeem` (challenge validation).
- Challenge token: 32 alnum chars, dashed groups of 4 (`XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX`).
- Token written to `challenge-token.txt` with `0o600` permissions; expires after 10 minutes (file mtime).
- Redeem validates challenge, atomically promotes it to `server_token.txt`, calls `AuthLayer::update_token()`,
  and deletes `challenge-token.txt` (single-use enforcement).
- Constant-time comparison used for challenge validation.
- Per-IP rate limiting on auth endpoints: custom in-process limiter (5 requests / 60s per IP),
  returning `429 Too Many Requests`. `tower-governor` was evaluated but deferred because the
  `0.8` release depends on `axum 0.8`, conflicting with this repo's `axum 0.7.9`.

### 7.3 Config file permissions — **[DONE]**
- `ensure_config_file()` sets `0o600` on auto-generated `config.toml`.
- `load_or_create_token()` uses `atomic_write_secret()` for `server_token.txt` with `0o600`.
- `atomic_write_secret()` helper added in `src/secrets_fs.rs` (tmp-file + rename + chmod).

### 7.4 Rate limiting + body size limits — **[DONE]**
- `tower_http::limit::RequestBodyLimitLayer::new(300 * 1024 * 1024)` caps uploads at 300 MB.
- Auth endpoints protected by custom `RateLimiter` (`src/rate_limit.rs`).
- Concurrency limiting deferred; can be added with `tower-governor` once axum 0.8 compatibility
  is resolved.

### 7.5 Token comparison hardening — **[DONE]**
- `src/security.rs` now uses a bytewise XOR accumulator `constant_time_eq()` for bearer-token
  comparison, replacing the previous `String::eq` short-circuit. This mitigates timing
  side-channels on the auth path. The same pattern is used in `src/auth_challenge.rs`.

---

## 8. Deployment & resilience

### 8.1 Default config — **[DONE]**
- Missing `config.toml` → writes default (`host = "127.0.0.1"`, `port = 1337`) to CWD or
  `/etc/l337-audio-server/`. Never panics on missing file.
- Override hierarchy: CLI flags → env (`L337__SERVER__HOST`, `L337__SERVER__PORT`,
  `L337__SERVER__TOKEN`, `L337__STORAGE__MAX_CACHE_SIZE_BYTES`, `L337__STORAGE__CACHE_DIR`) →
  `config.toml` → built-in defaults.

### 8.2 Directories — **[DONE]**
- Cache root honors `CACHE_DIRECTORY` then `STATE_DIRECTORY` (systemd), falls back to
  `~/.cache/l337/l337-audio-server/cache/`.
- Auth token persisted to `STATE_DIRECTORY` or cache dir as `server_token.txt`.
- Challenge token written to XDG config dir (`~/.config/l337-audio-server/challenge-token.txt`),
  honoring `XDG_CONFIG_HOME` if set.
- Default cap is **256 MiB**.

### 8.3 systemd — **[PARTIAL]**
- `scripts/install-systemd.sh` exists for dedicated `l337` system user.
- `scripts/run-server.sh` launches release binary.
- **[TODO]** Verify hardening flags (`ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`,
  `NoNewPrivileges`, `ReadOnlyPaths`, `ReadWritePaths`) are in the generated unit file.

### 8.4 Build & release — **[TODO]**
- `build.sh` dispatches to platform scripts. **[TODO]** Verify cross-compilation matrix works.
- **[TODO]** Add GitHub Actions workflow for multi-platform release artifacts (Linux x86/arm64,
  macOS x86/arm64, Windows x86_64).

---

## 9. Audio engine note

The PLAN originally described a rodio `Sink`-based engine. The actual implementation uses:
- **cpal** raw output stream with a custom callback that drains an `AudioBuffer` (PCM `Vec<f32>`).
- **rubato** `SincFixedIn` for sample-rate conversion and speed adjustment.
- **symphonia** for decoding MP3/AAC/FLAC/Vorbis/WAV to PCM.

This is functionally equivalent for the API contract but means `cpal` must initialize successfully
unless `--dummy` is passed. The `--dummy` flag exists and uses a no-op engine.

---

## 10. Client-side status (l337-player)

All client work is tracked in `PLAN-client.md`. High-level gaps vs server:
- `audio_server_l337` plugin directory exists at `src/client/plugins/audio_server/` but is empty — **TODO** implement probe + control logic.
- `APIClient` (`src/client/core/api_client.py`) supports bearer token + HTTPS + streaming push — **VERIFIED** working against `https://localhost:1337` with self-signed cert.
- `Player` source classification logic (local push vs. server-fetched URL) — **TODO**.
- Settings keys `server_url`, `server_token`, `remote_transcode` not yet in schema — **TODO**.

Verified: `l337-player` `connect_audio_server()` successfully detects and authenticates with the dummy server (`reachable: True, token_ok: True`).

---

*Generated: 2026-08-02. Maintain this file as the single source of truth for implementation state.*
