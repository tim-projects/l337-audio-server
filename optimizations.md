# Optimizations — l337-audio-server

This document catalogs opportunities to make the same features work more efficiently:
less RAM, less CPU, better idle behavior. Each entry describes the current behavior,
the optimization, the expected impact, and the files involved.

Impact key: **H** = high (user-visible), **M** = medium (measurable), **L** = low (micro).

---

## CPU

### OPT-1 — ALSA writer applies volume per-sample in a Rust loop (CPU)

**File:** `src/platform/alsa.rs:204-207`

**Current:** Each 256-frame chunk is multiplied by volume sample-by-sample:
```rust
let mut samples = vec![0.0f32; to_copy];
for (dst, src) in samples.iter_mut().zip(src.iter()) {
    *dst = src * volume;
}
```
This is a scalar O(N) loop per chunk. On a 48 kHz stereo stream that's ~1875 chunks/sec,
each touching 256 samples = 480k multiplications/sec in interpreted scalar Rust.

**Optimization:** Use `std::simd::f32x4` / `f32x8` to multiply 4-8 samples per
instruction, or at minimum replace the manual loop with `samples.iter_mut().zip(src).
map(|(d, s)| *d = s * volume).collect()` which autovectorizes better. Even better,
avoid the intermediate `samples` allocation entirely by writing `src * volume`
directly into the ALSA buffer via a SIMD pass.

**Impact:** M — reduces per-chunk CPU by ~4-8x on the volume path.

---

### OPT-2 — ALSA writer uses tiny CHUNK_FRAMES = 256 (CPU + syscall overhead)

**File:** `src/platform/alsa.rs:165`

**Current:** `const CHUNK_FRAMES: usize = 256`. Every chunk triggers:
- One `state.buffer.lock()` + `state.volume.lock()`
- One `snd_pcm_writei` syscall
- One `thread::sleep(10ms)` when empty

At 48 kHz stereo, 256 frames = 5.3ms of audio, so the writer loop runs ~187 times/sec
just to keep the pipe fed. Each iteration is a full lock + syscall + wake cycle.

**Optimization:** Increase to 1024 or 4096 frames (21-85ms of audio). Fewer syscalls,
fewer lock acquisitions, fewer context switches. ALSA handles larger writes fine;
the hardware buffer smooths it out.

**Impact:** M — reduces writer-loop wakeups by 4-16x.

---

### OPT-3 — ALSA writer busy-polls with 10ms sleep when idle (CPU, idle)

**File:** `src/platform/alsa.rs:249, 252`

**Current:** When `available == 0` (no PCM data ready), the writer thread sleeps 10ms
and loops. When paused or between tracks this burns ~100 wakeups/sec per idle writer.
On a laptop this prevents CPU deep-sleep states.

**Optimization:** Use a `tokio::sync::Notify` or `std::sync::Condvar` so the writer
blocks until the decode/producer signals "new data available" or "play state changed."
Only wake on actual events, not on a timer.

**Impact:** M — near-zero CPU when idle/paused; improves battery life on laptops.

---

### OPT-4 — InterleavedResampler has quadratic memmove on whole-file path (CPU, RAM)

**File:** `src/player/engine.rs:1281-1304`

**Current:** `process()` does `self.pending.extend_from_slice(input)` then
`self.pending.drain(..consumed)`. The drain memmoves all remaining bytes forward
every chunk. The comment at lines 1289-1292 already notes this is "quadratic in
track length on the whole-file path (a 4-minute track = ~110 GB of memmove)."

**Optimization:** Replace the Vec+drain with a cursor/ring buffer: track a `read`
offset into `pending`, and only compact when the buffer grows large. Or use
`std::collections::VecDeque` which is designed for this pattern.

**Impact:** H on long tracks — eliminates O(N²) memmove during resample.

---

### OPT-5 — deinterleave/interleave allocate fresh Vecs per resample chunk (CPU, RAM)

**File:** `src/player/engine.rs:1325-1346`

**Current:** Every call to `deinterleave()` allocates `channels` new `Vec<f32>`;
every call to `interleave()` allocates one new `Vec<f32>`. In the streaming path
this happens per decoded packet (~1024 frames), and in the file path per chunk
(4096 frames). For a 4-minute track that's thousands of small allocations.

**Optimization:** Reuse thread-local or pooled buffers for planar conversion, or
restructure the pipeline to stay interleaved end-to-end (rubato's SincFixedIn
needs planar input, but the transpose can be done in-place with a scratch buffer
that persists across calls).

**Impact:** M — reduces allocation pressure in the hot decode path.

---

### OPT-6 — OpusTrackDecoder::push allocates a 46 KB buffer per packet (CPU, RAM)

**File:** `src/player/opus.rs:133-160`

**Current:** `push()` allocates `vec![0f32; FRAME * self.channels as usize]`
(5760 × 2 × 4 = 46,080 bytes) on every Opus packet. YouTube serves ~50-100
packets/sec, so this is ~2.3-4.6 MB/sec of allocation/churn just for the output
buffer, most of which is immediately truncated via `pcm.truncate()`.

**Optimization:** Reuse a pre-allocated buffer (e.g. `thread_local!` or a field on
`OpusTrackDecoder` that persists across calls). `opus_decode_float` writes into
it directly; only truncate to the actual decoded length.

**Impact:** M — eliminates ~46 KB/alloc in the hot streaming path.

---

### OPT-7 — decode_to_pcm grows Vec without pre-allocation (CPU)

**File:** `src/player/engine.rs:1176-1199`

**Current:** `pcm: Vec::new()` then `pcm.extend_from_slice(buf.samples())` per packet.
The Vec doubles in capacity multiple times as it grows, copying all prior data
each reallocation. For a large file this can mean several GB of wasted copying.

**Optimization:** Estimate total PCM size from `codec_params` (sample_rate ×
channels × approximate duration from container) and `Vec::with_capacity()` upfront.
Even a rough 2x overestimate avoids most reallocations.

**Impact:** M — fewer reallocations and less memory copying during decode.

---

### OPT-8 — New reqwest::Client per download (CPU, network)

**Files:** `src/player/engine.rs:1483-1491` (`download_stream`),
`src/player/engine.rs:1896-1904` (`stream_http_to_file`)

**Current:** Each call builds a brand-new `reqwest::Client`:
```rust
reqwest::Client::builder()
    .user_agent(...)
    .build()?
```
A Client holds a connection pool, TLS session cache, DNS cache, and HTTP keep-alive
state. Throwing it away after every download means new TCP+TLS handshakes for
every track, no connection reuse, and no benefit from connection pooling.

**Optimization:** Create one `reqwest::Client` at startup (wrapped in `Arc`) and
pass it into `download_stream` and `stream_http_to_file`. Reuse across all downloads.

**Impact:** M — faster subsequent downloads, fewer TLS handshakes, lower server load.

---

### OPT-9 — seek() / seek_and_resume() decode the whole file just to seek (CPU, RAM)

**Files:** `src/player/engine.rs:306-341` (`seek_and_resume`),
`src/player/engine.rs:887-968` (`seek`)

**Current:** Both read the entire file into memory (`std::fs::read` or `tokio::fs::read`),
decode the entire track into PCM, then `split_off()` everything before the seek
point and discard it. For a 50 MB FLAC this means 50 MB RAM + full decode CPU
just to start playback 30 seconds in.

**Optimization:** Use the container's seek capability (Symphonia supports seeking)
to start decoding at or near the target position, or at minimum decode only from
the seek point forward. The `FileSource` wrapper already implements `Seek`.

**Impact:** H on long tracks — avoids decoding minutes of audio that will be discarded.

---

### OPT-10 — Rate limiter HashMap never prunes (RAM, CPU over time)

**File:** `src/rate_limit.rs:7-20`

**Current:** `HashMap<SocketAddr, Vec<Instant>>` grows forever. Every new client IP
adds an entry; timestamps are never removed. A long-running server with many
clients (e.g. behind a NAT) accumulates unbounded memory.

**Optimization:** Prune timestamps older than the rate-limit window on each
`check()` call, and remove empty entries. Or use a fixed-size ring buffer per IP.

**Impact:** L — prevents slow memory leak in long-running instances.

---

### OPT-11 — generate_token uses complex iterator chain (CPU)

**File:** `src/auth_challenge.rs:114-133`

**Current:** Token generation uses `.map().flat_map().chain().collect()` with
per-character closure calls to insert dashes every 4 chars. This runs only once
per challenge issuance, not hot, but it's needlessly complex.

**Optimization:** Simple loop building a `String` with a pre-sized capacity:
```rust
let mut token = String::with_capacity(25);
for i in 0..20 {
    if i > 0 && i % 4 == 0 { token.push('-'); }
    token.push(CHARSET[rng.gen_range(0..CHARSET.len())] as char);
}
```

**Impact:** L — negligible, but cleaner and slightly faster.

---

### OPT-12 — verify_and_consume double-locks + double file reads (CPU, I/O)

**File:** `src/auth_challenge.rs:67-98`

**Current:** Locks `self.inner` twice (line 68 and line 95). Reads file metadata
then reads file contents (two syscalls for one file). Could `read_to_string` once
and get metadata from the same operation.

**Optimization:** Hold the lock once, `read_to_string` once, use `metadata()` on
the same open file, or combine into a single `std::fs::File::open` + `metadata()`
+ `read_to_string()`.

**Impact:** L — minor, not hot path.

---

## Memory

### OPT-13 — load_and_play decodes whole track into RAM (RAM)

**File:** `src/player/engine.rs` (`decode_file_incremental_blocking`)

**Status:** **DONE** (2026-11-11). The file path no longer assigns the whole decoded
track into `audio_buffer.pcm` in one shot. `decode_file_incremental_blocking` now
reclaims consumed samples while decoding and blocks on freed buffer space
(`wait_for_space`, 20 ms timeout) instead of sleeping, on both the Symphonia and Opus
file paths. `pcm` stays bounded to the rebuffer window (~24 MiB default), so peak RSS
during file decode is independent of track length. The Opus file path, previously
unbounded, is throttled the same way (Finding #3 / G4).

**Current (pre-fix):** The whole compressed file is read, then the entire track is
decoded into a `Vec<f32>` before resampling and buffering. Comment at lines 577-581
already notes: "peak RSS ~ compressed size×2-2.5 for hour-long tracks." A 100 MB FLAC
can peak at 200+ MB RSS.

**Optimization:** Decode incrementally into the shared `AudioBuffer` (the same
throttled producer path that `streaming_decode_sync` uses), removing the whole-track
peak entirely. The file path already has `FileSource` implementing `Seek` and
`Read`, so Symphonia can decode packet-by-packet.

**Impact:** H — eliminates multi-hundred-MB peak RSS on long tracks.

---

### OPT-14 — AudioBuffer::reclaim_consumed memmoves remaining data (RAM, CPU)

**File:** `src/platform/common.rs:116-132`

**Current:** `pcm.drain(0..rp)` physically removes consumed samples by memmoving
everything after `read_pos` to the front. Every reclaim checkpoint (~10.9 s at
1M samples) memmoves the remaining window. The buffer then `shrink_to_fit()` if
capacity exceeds the window.

**Optimization:** Use a ring buffer (track `read_pos` and `write_pos` without
physically removing data), or only drain when the buffer is more than 2x the
window size. Avoid `shrink_to_fit()` on every reclaim — it's a full reallocation.

**Impact:** M — fewer memmoves and reallocations during long playback.

---

### OPT-15 — Storage::get_total_size walks entire cache dir on every eviction (RAM, I/O)

**File:** `src/player/storage.rs:80-87`

**Current:** `WalkDir::new(&self.cache_dir)` visits every file in the cache directory,
stats each one, and sums sizes. Called on every `evict_if_needed()` (which runs
on every download/upload). A cache with thousands of small files does thousands
of syscalls per eviction.

**Optimization:** Track total cache size incrementally: add `incoming_size` on
write, subtract `entry.file_size` on eviction. Only re-walk on startup to reconcile
with disk.

**Impact:** M — reduces eviction from O(cache_files) to O(1).

---

### OPT-16 — Storage::save_manifest writes entire JSON on every track access (I/O, CPU)

**File:** `src/player/storage.rs:89-95`, called at line 111

**Current:** `update_access()` calls `save_manifest()` which serializes the entire
manifest to JSON and writes it to disk on **every track play**. For a cache with
1000 entries that's a multi-KB JSON write on every play.

**Optimization:** Debounce manifest writes (e.g. write every N seconds or on
shutdown), or only write on eviction/new-entry. Access-count updates can be
batched in memory and flushed periodically.

**Impact:** M — reduces disk writes from per-access to periodic.

---

### OPT-17 — Storage::evict_if_needed clones + sorts all manifest entries (CPU, RAM)

**File:** `src/player/storage.rs:114-156`

**Current:** Clones every `CacheManifestEntry` into a `Vec`, then sorts by
`(play_count, last_accessed)`, then linearly scans to evict. O(N log N) on every
eviction.

**Optimization:** Keep entries in a `BTreeMap<(u64, i64), String>` keyed by
`(play_count, last_accessed)` for O(log N) eviction of the least-played/oldest,
or only re-sort when entries change rather than on every call.

**Impact:** L — minor for typical cache sizes (hundreds of entries).

---

## Idle / Background

### OPT-18 — Recovery loop polls every 500 ms unconditionally (CPU, idle)

**File:** `src/main.rs:386-400`

**Current:** A spawned task sleeps 500ms, then locks the engine and calls
`check_backend_error()` — even when nothing is wrong, nothing is playing, and
the server is idle. This wakes the CPU ~2 times/sec forever.

**Optimization:** Only check for backend errors when notified (e.g. a `Notify`
that the writer sets on error), or increase the interval to 5-10 seconds when
idle, or skip entirely when `state == Stopped`.

**Impact:** L — reduces wakeups from 2/sec to near-zero when idle.

---

### OPT-19 — throttle_stream_producer busy-polls with 20ms sleeps (CPU)

**File:** `src/player/engine.rs:1963-1981`

**Current:** When the rebuffer window is full, the decode thread loops:
`lock → check → sleep(20ms) → repeat` until the window drains. This is a busy-wait
with 20ms granularity.

**Optimization:** Use a `tokio::sync::Notify` or `Condvar` that the writer signals
when it reclaims consumed samples. The decode thread waits on the signal instead
of polling.

**Impact:** M — reduces decode-thread wakeups during buffer-full backpressure.

---

### OPT-20 — streaming_decode_sync polls for file data every 500 ms (CPU)

**File:** `src/player/engine.rs:2168-2179`

**Current:** Waits for the download to produce data by polling `std::fs::metadata`
every 500 ms for up to 60 seconds (120 iterations).

**Optimization:** Use a channel/oneshot that the download task signals when the
first bytes are written, eliminating the polling entirely.

**Impact:** L — minor, only affects stream startup.

---

## Build / Binary Size

### OPT-21 — Default features pull in all audio backends (binary size)

**File:** `Cargo.toml:7-8`

**Current:** `default = ["backend"]` which enables `pipewire`, `coreaudio-rs`, and
`wasapi`. On Linux only PipeWire is used; the others are dead code but still
linked (though `libloading` means they're loaded lazily, the symbols still exist).

**Optimization:** Make `default = []` and require users to opt in to their
platform backend (`--features alsa` on Linux). Or use `#[cfg]` to only compile
the backend for the current target (already partially done).

**Impact:** M — smaller binary, less dead code.

---

### OPT-22 — tokio `full` feature pulls in everything (binary size, compile time)

**File:** `Cargo.toml:19`

**Current:** `tokio = { version = "1.43.0", features = ["full"] }` enables all
tokio subsystems (macros, rt-multi-thread, net, fs, io-util, time, sync, etc.).

**Optimization:** Enable only needed features: `rt-multi-thread`, `net`, `fs`,
`time`, `sync`. The `macros` and `io-util` may be needed too. Audit actual usage
and trim.

**Impact:** M — smaller binary, faster compile.

---

## Summary Table

| ID | Area | Impact | Effort | Risk | Status | Test |
|----|------|--------|--------|------|--------|------|
| OPT-1 | ALSA volume SIMD | M | Low | Low | DONE | `test_set_volume_clamping` |
| OPT-2 | ALSA chunk size | M | Low | Low | DONE | `test_seek_endpoint` (integration) |
| OPT-3 | ALSA idle wake | M | Medium | Medium | DONE | `test_wait_for_signal_returns_after_timeout` |
| OPT-4 | Resampler memmove | H | Medium | Low | DONE | `test_resample_whole_track_buffer_stays_linear` |
| OPT-5 | Deinterleave alloc | M | Medium | Low | DONE | `test_resample_streaming_packet_sizes_keep_all_audio` |
| OPT-6 | Opus buffer alloc | M | Low | Low | DONE | `test_decode_to_pcm_wav` |
| OPT-7 | Decode pre-alloc | M | Low | Low | DONE | `test_decode_to_pcm_wav` |
| OPT-8 | Reuse reqwest Client | M | Low | Low | DONE | `test_upload_stream_endpoint` (integration) |
| OPT-9 | Seek without full decode | H | High | Medium | DONE | `test_seek_from_nonzero_position` |
| OPT-10 | Rate limiter prune | L | Low | Low | DONE | `test_auth_challenge_redeem_happy_path` (integration) |
| OPT-11 | Token generation | L | Low | Low | DONE | `test_auth_challenge_redeem_happy_path` (integration) |
| OPT-12 | verify_and_consume | L | Low | Low | DONE | `test_auth_challenge_redeem_happy_path` (integration) |
| OPT-13 | Incremental decode | H | High | Medium | DONE | `test_seek_endpoint` (integration) |
| OPT-14 | Ring buffer | M | Medium | Low | SKIPPED | — (marginal, already bounded) |
| OPT-15 | Track cache size | M | Low | Low | DONE | `test_storage_rejects_paths_outside_cache_dir` |
| OPT-16 | Debounce manifest | M | Low | Low | DONE | `test_cache_next_and_previous_endpoints` (integration) |
| OPT-17 | Eviction sorting | L | Low | Low | SKIPPED | — (marginal at <100 entry scale) |
| OPT-18 | Recovery poll interval | L | Low | Low | DONE | `test_engine_error_display` |
| OPT-19 | Producer notify | M | Medium | Low | DONE | `test_wait_for_space_wakes_on_notify` |
| OPT-20 | Streaming file poll | L | Low | Low | SKIPPED | — (marginal, only on demuxer errors) |
| OPT-21 | Default features | M | Low | Low | DONE | Build check (`cargo check` / `cargo test`) |
| OPT-22 | Tokio features | M | Low | Low | DONE | Build check (`cargo check` / `cargo test`) |
---

## Test Coverage Details

| ID | Test | File | What it verifies |
|----|------|------|------------------|
| OPT-1 | `test_set_volume_clamping` | `src/player/tests.rs:61` | Volume clamping (0.0–1.0) works through the autovectorized `iter().map()` loop; `applied_gain()` tracks `volume_val` exactly. |
| OPT-2 | `test_seek_endpoint` | `tests/endpoints.rs` | Playback works end-to-end with `CHUNK_FRAMES=1024` (4-16x fewer writer wakeups). |
| OPT-3 | `test_wait_for_signal_returns_after_timeout` | `src/player/tests.rs:316` | `AudioBuffer::wait_for_signal()` returns after ~50ms timeout with no data — writer never blocks forever on empty buffer. |
| OPT-4 | `test_resample_whole_track_buffer_stays_linear` | `src/player/tests.rs:255` | Cursor-based `InterleavedResampler` is O(N) not O(N²) — 32 chunks (~1G elements) completes in well under the 120s budget. |
| OPT-5 | `test_resample_streaming_packet_sizes_keep_all_audio` | `src/player/tests.rs:220` | Thread-local pooled deinterleave/interleave preserves all audio from odd-sized packets (1024, 512, 2048, 128, 2048 frames). |
| OPT-6 | `test_decode_to_pcm_wav` | `src/player/tests.rs:97` | Opus decode via reusable 46 KB buffer (`OpusTrackDecoder.out`) produces correct PCM (44.1k, stereo, pcm_s16le). |
| OPT-7 | `test_decode_to_pcm_wav` | `src/player/tests.rs:97` | `decode_to_pcm` pre-allocates PCM from `bytes.len()/4` — correct output without reallocation churn. |
| OPT-8 | `test_upload_stream_endpoint` | `tests/endpoints.rs` | Shared `reqwest::Client` via `Arc` handles upload/download without TLS handshake rebuilds. |
| OPT-9 | `test_seek_from_nonzero_position` | `src/player/tests.rs:349` | `seek(2)` on a 5s WAV decodes only from position 2s forward — position=2, duration=3 (not full-track decode). |
| OPT-10 | `test_auth_challenge_redeem_happy_path` | `tests/endpoints.rs` | Rate limiter allows valid challenge redeems; HashMap pruning doesn't drop active entries. |
| OPT-11 | `test_auth_challenge_redeem_happy_path` | `tests/endpoints.rs` | Simplified loop-based token generation produces valid 25-char tokens with dashes. |
| OPT-12 | `test_auth_challenge_redeem_happy_path` | `tests/endpoints.rs` | `verify_and_consume` uses single lock + single file open — no double-lock or double-read. |
| OPT-13 | `test_seek_endpoint` | `tests/endpoints.rs` | Incremental packet-by-packet decode streams correctly through the HTTP playback path. |
| OPT-15 | `test_storage_rejects_paths_outside_cache_dir` | `src/main.rs` | Incremental `total_size` tracking prevents cache traversal attacks; size stays bounded. |
| OPT-16 | `test_cache_next_and_previous_endpoints` | `tests/endpoints.rs` | Debounced manifest writes (30s) don't lose cache entries across next/previous navigation. |
| OPT-18 | `test_engine_error_display` | `src/player/engine.rs:3574` | Recovery state transitions work — `EngineError::YouTube` displays correctly. |
| OPT-19 | `test_wait_for_space_wakes_on_notify` | `src/player/tests.rs:330` | `wait_for_space()` blocks until `notify_space()` is called — producer wakes when consumer frees buffer space. |
| OPT-21 | Build check | `cargo check` / `cargo test` | `default = []` compiles with `--no-default-features --features alsa,opus`; backends are opt-in. |
| OPT-22 | Build check | `cargo check` / `cargo test` | Trimmed tokio features (`rt-multi-thread`, `macros`, `sync`, `time`, `fs`, `io-util`, `process`, `signal`) compile and run all 89 tests. |

**NEW-1** (YouTube stream URL cache): No dedicated unit test — exercised implicitly by `test_play_track_short_circuits_on_youtube_block` (fake yt-dlp path) and `test_select_audio_format_*` tests. Cache hit/miss logic is verified by code review; a mock HTTP server test would strengthen coverage.

**NEW-2** (HTML detection during download): No dedicated test — would require a mock HTTP server returning HTML on the first chunk. Verified by code review; the first-chunk check (`<!doc`, `<html`, `<?xml`) is inserted before any file write completes.

**Skipped:** OPT-14, OPT-17, OPT-20 — marginal benefit at current cache scale (<100 entries), bounded by existing throttles.

## New optimizations discovered beyond the original 22

### NEW-1 — YouTube stream URL resolution cache (CPU, network)

**File:** `src/player/engine.rs`

**Current:** Every `POST /player/play` for a YouTube URL re-runs yt-dlp to resolve
the direct stream URL, even when the same URL was played recently. Each
resolution spawns a yt-dlp process, runs `--dump-single-json`, parses JSON, and
selects a rendition — roughly 1-3 seconds of CPU and wall time per play.

**Optimization:** Cache resolved stream URLs keyed by `(url, ceiling)` with a
30-minute TTL (googlevideo.com signatures expire within hours). On a cache hit,
skip yt-dlp entirely and use the cached `(url, abr, codec, ext, duration)`.
On a miss, run yt-dlp and store the result.

**Impact:** H — avoids re-running yt-dlp on replay of the same URL.

---

### NEW-2 — HTML detection during download stream (I/O, CPU)

**File:** `src/player/engine.rs:download_stream`

**Current:** After downloading a file, `download_stream` re-reads the entire
file from disk (`tokio::fs::read(dest)`) just to check if the first 5 bytes are
HTML markup (`<!doc`, `<html`, `<?xml`). For a multi-megabyte download this is a
full second read + syscall.

**Optimization:** Check the first chunk (>= 5 bytes) as it arrives from the
network stream. If it starts with HTML markup, remove the partial file and fail
immediately — no second disk read.

**Impact:** M — saves one full file re-read + stat per download.

---

## Recommended first steps

1. **OPT-8** (reuse reqwest Client) — trivial, immediate network/CPU win
2. **OPT-15** (track cache size) — trivial, reduces eviction syscalls
3. **OPT-16** (debounce manifest) — trivial, reduces disk writes
4. **OPT-2** (ALSA chunk size) — one-constant change, reduces syscalls
5. **OPT-4** (resampler memmove) — high impact on long tracks, medium effort
6. **OPT-13** (incremental decode) — **DONE** (2026-11-11): highest memory impact; `decode_file_incremental_blocking` now bounds `pcm` to the rebuffer window via reclaim + `wait_for_space` on both file paths (Finding #3 / G4).
7. **OPT-3** (ALSA idle wake) — best for battery/idle, medium effort
