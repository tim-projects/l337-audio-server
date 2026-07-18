# L337 Audio Server — Implementation Plan

> This document is the **living plan** for the Rust `l337-audio-server`. It supersedes the original
> greenfield "copy-paste prompt" in the repo history. The server is already substantially implemented
> (Axum + Tokio REST on :1337, rodio sink, `current/next/prev.stream` slots, persistent LRU cache with a
> `track_id` manifest). This plan records the **remaining work** to make the server the primary audio
> output for the `l337-player` Python client, including **cross-device playback (LAN + WWAN)**.

## 0. Current architecture (what exists)
- `src/main.rs` — Axum app, binds `config.toml [server] host/port`, wires 9 routes, creates `StorageManager` (hardcoded 500 MB) + `PlayerEngine`.
- `src/api/models.rs` — `Track{track_id, stream_url, title?, artist?, duration?}`, `SpeedPayload`, `PoolSettings`, `PlayerStatus{state(uppercase enum), volume, speed, current_track?, disk_pool_utilization_bytes, next_cached, prev_cached}`, `CacheManifestEntry{track_id, file_size, last_accessed, play_count}`.
- `src/api/handlers.rs` — `play`, `pause`, `next`, `previous`, `cache_next`, `cache_previous`, `set_speed`, `get_status`, `set_settings`. State wrapped in `SendableEngine(Mutex<PlayerEngine>)` with `unsafe impl Send/Sync`.
- `src/player/engine.rs` — `PlayerEngine` (rodio `Sink`), `play_track` (downloads via `reqwest::get`), `load_and_play`, `pause/resume/stop`, `set_speed`, `trigger_next/previous`, `get_status`, `download_stream` (network only).
- `src/player/storage.rs` — `StorageManager`: cache dir, manifest load/save, `update_access`, `evict_if_needed` (LRU by `last_accessed`, protects active slots).

## 1. Goals
1. Make the server the **main audio output** for the client (client drives it via HTTP API).
2. Support **separate devices** (client + server on different machines, LAN **and** WWAN).
3. Allow the client to deliver audio the server can't fetch: **direct file push** and **client-side transcoding** (bandwidth reduction).
4. Configurable **cache size** (default **256 MB**) at `~/.cache/l337/l337-audio-server/cache/`, evicted by **least-played / LRU**.
5. **Secure by default** for LAN + WWAN: built-in **TLS (HTTPS)** + **shared-token** auth.

## 2. Shared API contract (client ↔ server)
- `Track` payload: `{track_id:str, stream_url:str, title?:str, artist?:str, duration?:u64}`.
  `track_id` and `stream_url` are **required** (400 if missing/empty). `track_id` = client-generated stable hash (sha1 of canonical stream URL).
- `state` serializes **lowercase**: `playing | paused | stopped`.
- `PlayerStatus`: `{state, volume, speed, current_track?, disk_pool_utilization_bytes, next_cached, prev_cached, position_sec?, duration_sec?}`.
- Auth: every request (except `GET /health`) requires header `Authorization: Bearer <token>` (alias `X-L337-Token`). 401 otherwise.
- All endpoints served over **HTTPS** when TLS is configured.

### Routes
| Method | Endpoint | Body | Notes |
|---|---|---|---|
| GET | `/health` | — | Unauthenticated liveness probe |
| POST | `/player/play` | `Track` | Download `stream_url` (or `file://`/local path) → `current.stream`, play |
| POST | `/player/play/stream` | raw bytes + `X-Track-Id` (+`X-Title`/`X-Artist`) | Client pushes audio; write straight to `current.stream` |
| POST | `/player/cache/next` | `Track` | Background download → `next.stream` |
| POST | `/player/cache/next/stream` | raw bytes + `X-Track-Id` | Push → `next.stream` |
| POST | `/player/cache/previous` | `Track` | Background download → `prev.stream` |
| POST | `/player/cache/previous/stream` | raw bytes + `X-Track-Id` | Push → `prev.stream` |
| POST | `/player/next` | — | Swap to next |
| POST | `/player/previous` | — | Swap to previous |
| POST | `/player/pause` | — | Toggle pause |
| POST | `/player/seek` | `{position:u64}` | Seek to second |
| POST | `/player/speed` | `{speed:f32}` | Playback speed |
| POST | `/player/volume` | `{volume:f32}` | Volume (0.0..1.0) |
| PUT  | `/player/settings` | `{max_disk_pool_bytes:u64}` | Adjust cache cap at runtime |
| GET  | `/player/status` | — | `PlayerStatus` |

## 3. Server changes (TODO)

### 3.1 Cache root + size (config-driven)
- `config.toml`: add `[storage] max_cache_size_bytes` (default `268435456` = 256 MiB) and use cache root
  `~/.cache/l337/l337-audio-server/cache/` (override via `[storage] cache_dir` optional).
- `StorageManager::new(max_pool_size, cache_dir?)` reads config; `main.rs` passes parsed values.
- Keep `manifest.json` in the cache dir.

### 3.2 Play-count / LRU eviction
- `evict_if_needed` already increments `play_count` via `update_access`. Change sort to
  **least-played, then oldest**: `entries.sort_by_key(|e| (e.play_count, e.last_accessed))`.
- `update_access` is called on every `play_track` (already) — ensure it's also called when a pushed
  stream is played.
- Protect active slots (`current`/`next`/`prev` manifest keys) from eviction (already done).

### 3.3 `file://` / local-path passthrough
- In `download_stream`, if `url` starts with `file://` or is an existing absolute path the server can
  read, copy via `tokio::fs::copy` instead of `reqwest::get`.

### 3.4 Streaming upload endpoints
- Add axum handlers consuming the request `Body` (streaming) + headers `X-Track-Id`, `X-Title`,
  `X-Artist`. Write chunks to the target slot (`current`/`next`/`prev`) under the engine lock, applying
  eviction first so the incoming stream never exceeds `max_pool_size`. Update manifest `play_count` on
  play. Mirror the play/cache semantics of the non-stream routes.
- Validate `X-Track-Id` present (400 if missing).

### 3.5 Seek + volume + status enrichment
- `engine.seek(position: u64)` using rodio `Sink::seek` (or `Seek` trait) — add `POST /player/seek`.
- `engine.set_volume(f32)` (clamp) — add `POST /player/volume` (already has `volume` field; wire it).
- `PlayerStatus`: add `position_sec: Option<u64>` (`Sink::get_pos`) and `duration_sec: Option<u64>`
  (decode source duration). Serialize `state` lowercase.
- `play`/`cache_*`: 400 when `stream_url`/`track_id` empty.

### 3.6 Security: TLS + token
- `config.toml [server]`: `host` (allow `0.0.0.0`), `port`, `token` (optional), `tls_cert`, `tls_key`
  (optional).
- If `token` unset → generate random token at first run, persist to config, log it for the user.
- If `tls_cert`/`tls_key` unset → auto-generate a self-signed cert into the config dir, log fingerprint.
  Serve via `rustls` (`tokio-rustls`/`axum-server` or `hyper-rustls`).
- Token auth **middleware** on all routes except `GET /health` (401 otherwise).
- `reqwest` → enable `rustls-tls` feature for pure-Rust (Android) builds.

### 3.7 Misc
- Remove `unsafe impl Send/Sync for SendableEngine` if `rodio::Sink` is `Send+Sync` (verify).

## 4. Client changes (see PLAN-client.md)
- New `audio_server_l337` plugin (probe-only, highest auto-detect priority): probes `GET {server_url}/health`
  with token; `get_api_endpoint()` returns `server_url` when reachable.
- `APIClient`: HTTPS + bearer token; streaming push to `*/stream` endpoints; `X-Track-Id` header.
- `Player`: source classification — network URL server can fetch → send `stream_url`; local file / server
  can't fetch → **push** (optionally transcoded via ffmpeg/yt-dlp, config-gated).
- Payload normalization: set `track_id` (sha1) + `stream_url`; status normalization (lowercase state,
  `position_sec`/`duration_sec`); seek/volume routing.
- Settings: `server_url`, `server_token`, `remote_transcode`.

## 5. Sequencing
1. Cache root/size (3.1) + play-count eviction (3.2) + `file://` passthrough (3.3).
2. Streaming `*/stream` upload endpoints (3.4).
3. Seek + volume + status enrichment + lowercase state + 400 validation (3.5).
4. TLS + token middleware + `/health` (3.6).
5. Client: `APIClient` HTTPS+token (4) → `l337` plugin → source classification/push/transcode → normalization.
6. E2E: localhost (Rust primary, fallback avoided) → LAN (separate device, TLS+token) → WWAN.

## 6. Deployment & resilience
- **Default config:** if `config.toml` is absent on startup, the server writes a default
  (`host = "127.0.0.1"`, `port = 1337`) and continues — it never panics on a missing file.
  Override via `config.toml`, env (`L337__SERVER__HOST`, `L337__SERVER__PORT`,
  `L337__SERVER__TOKEN`, `L337__STORAGE__MAX_CACHE_SIZE_BYTES`, `L337__STORAGE__CACHE_DIR`),
  or CLI flags.
- **Directories:** the cache root honors `CACHE_DIRECTORY` then `STATE_DIRECTORY` (set by
  systemd) and falls back to `~/.cache/l337/l337-audio-server/cache/`. The persisted auth
  token is written to `STATE_DIRECTORY` when provided, else the same cache dir. Default cap
  is **256 MiB**.
- **systemd:** `scripts/install-systemd.sh` installs the server as a dedicated, unprivileged
  `l337` system user (MPD-style: `User=l337`/`Group=l337`, audio group, hardened
  `ProtectSystem=strict`, `CacheDirectory`/`StateDirectory`/`ConfigurationDirectory`). A
  per-user instance is available via `./scripts/install-systemd.sh --user`. `scripts/run-server.sh`
  launches the release binary.
