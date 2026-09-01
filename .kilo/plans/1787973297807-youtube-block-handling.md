# YouTube Block Error Relay

## Context
When YouTube IP-blocks the server, `play_track` falls back from streaming to full download — a second guaranteed-failed yt-dlp call. The server burns quota, delays the user, and returns a generic error. The client has no way to distinguish "YouTube blocked us" from "track is private".

## Goal
Detect block error codes from yt-dlp output and relay them to the client as structured errors. The client owns block tracking, rate limiting, and cooldowns. The server stays lightweight.

## Decisions

### 1. Block detection (server-side, stateless)
In `src/player/engine.rs`, parse yt-dlp stderr after every invocation:
- `HTTP Error 429` → `block_type: "rate_limit"`
- `captcha` / `playerCaptchaViewModel` → `block_type: "captcha"`
- `403` / `Cloudflare` → `block_type: "bot_detected"`
- `No video formats found` / `Skipping player response` → `block_type: "ip_blocked"`

No retry, no cooldown, no in-memory block state on the server.

### 2. Structured error response (server-side)
When a block is detected, return HTTP **502** with JSON body:
```json
{
  "error": "youtube_blocked",
  "block_type": "rate_limit",
  "message": "YouTube is rate-limiting this server"
}
```

When a non-block error occurs, return the existing generic error format.

### 3. Short-circuit fallback (server-side)
In `play_track` (engine.rs lines 138-149): if `start_streaming_playback` returns a block error, **do NOT fall back to `download_stream`**. Return the block error immediately. This prevents the second guaranteed-failed yt-dlp call.

### 4. Client responsibility (out of scope for this repo)
l337-player will:
- Parse `block_type` from the 502 JSON via existing `_extract_server_error()`
- Persist block events in SQLite (`sources.last_error`, `sources.last_error_at`)
- Track first/last block timestamps per source URL
- Enforce its own rate limiting and cooldowns before requesting YouTube URLs

## Files to change

### `src/player/engine.rs`
- Add `YouTubeBlockType` enum (`RateLimit`, `Captcha`, `BotDetected`, `IpBlocked`)
- Add `detect_youtube_block(stderr: &str) -> Option<YouTubeBlockType>`
- Add `YouTubeError { block_type: YouTubeBlockType }`
- In `download_via_ytdlp`: parse stderr after proc completes; if block detected, return `YouTubeError`
- In `resolve_youtube_stream_url`: same detection
- In `start_streaming_playback`: if block detected, return `YouTubeError` instead of falling through
- In `play_track` (YouTube path): if `start_streaming_playback` returns `YouTubeError`, return it directly — no fallback to `download_stream`

### `src/api/handlers.rs`
- Map `YouTubeError` to HTTP 502 with JSON body `{ "error": "youtube_blocked", "block_type": "...", "message": "..." }`
- Non-YouTube errors keep existing behavior

## Validation
- Unit tests for `detect_youtube_block` with sample yt-dlp stderr strings (429, CAPTCHA, 403, No formats)
- Unit test: block detected in streaming path → `play_track` returns error without calling `download_stream`
- CI: `cargo test` passes

## Risks
- yt-dlp output formats change between versions → detection regexes need maintenance

## Out of scope
- Server-side retry, cooldown, or rate limiting
- Client-side block tracking (tracked in l337-player)
- Cookie auto-refresh, CAPTCHA solving, proxy rotation

## Client coordination artifact

The server implementation must be paired with a client-side prompt file so the l337-player agent understands the contract without reading server code.

Create `../l337-player/errors.md` with the following content:

```markdown
# YouTube Block Errors — Client Contract

## Problem
When YouTube IP-blocks the l337-audio-server, the server detects the block and returns a structured 502 error. The client must recognize this error, persist it, and enforce its own rate limiting / cooldowns. The server does NOT manage blocks or retries.

## Server behavior (already implemented in l337-audio-server)
- yt-dlp stderr is parsed for block signatures after every invocation.
- Block types: `rate_limit` (429), `captcha`, `bot_detected` (403/Cloudflare), `ip_blocked` (No video formats).
- When a block is detected, the server returns HTTP 502 with JSON body:
  {
    "error": "youtube_blocked",
    "block_type": "<one of the block types above>",
    "message": "YouTube is rate-limiting this server"
  }
- The server short-circuits its own fallback: if streaming fails with a block, it does NOT fall back to full download. It returns the 502 immediately.
- Non-block errors continue to use the existing generic error format.

## Client responsibility
1. Parse `block_type` from the 502 JSON via existing `_extract_server_error()` in `playback_engine.py`.
2. Persist block events in SQLite:
   - `sources.last_error` = block type string (e.g. "rate_limit")
   - `sources.last_error_at` = timestamp of the block
3. Before resolving a YouTube URL, check if the source has a recent block:
   - If `last_error` matches a block pattern and `now - last_error_at < cooldown`, skip or delay the request.
   - Cooldowns: rate_limit/captcha/bot_detected → 5 minutes; ip_blocked → 30 minutes.
4. Track block duration per source URL:
   - Store first block timestamp and last block timestamp.
   - Compute duration: `last_block_at - first_block_at`.
   - If blocks persist beyond the cooldown, surface "YouTube blocked for {duration}" to the user.
5. Surface block state to UI:
   - When all candidates fail due to blocks, show "YouTube blocked since {first_block_at} ({duration} ago)" instead of generic "playback failed".
6. Continue using existing rate limiting (1 req/60s per source) to reduce self-inflicted 429s.

## Files to change (l337-player)
- `src/client/core/playlists_db.py`: add `last_error_at REAL` column to `sources` table.
- `src/client/tui/playback_engine.py`: extend `_extract_server_error` to recognize `youtube_blocked`, store block events via `record_source_result`, add block-duration display.
```

## Validation
- Unit tests for `detect_youtube_block` with sample yt-dlp stderr strings (429, CAPTCHA, 403, No formats)
- Unit test: block detected in streaming path → `play_track` returns error without calling `download_stream`
- CI: `cargo test` passes

## Risks
- yt-dlp output formats change between versions → detection regexes need maintenance

## Out of scope
- Server-side retry, cooldown, or rate limiting
- Client-side block tracking (tracked in l337-player)
- Cookie auto-refresh, CAPTCHA solving, proxy rotation
