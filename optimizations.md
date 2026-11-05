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

**File:** `src/player/engine.rs:521-620`

**Current:** The whole compressed file is read, then the entire track is decoded
into a `Vec<f32>` before resampling and buffering. Comment at lines 577-581 already
notes: "peak RSS ~ compressed size×2-2.5 for hour-long tracks." A 100 MB FLAC can
peak at 200+ MB RSS.

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

| ID | Area | Impact | Effort | Risk |
|----|------|--------|--------|------|
| OPT-1 | ALSA volume SIMD | M | Low | Low |
| OPT-2 | ALSA chunk size | M | Low | Low |
| OPT-3 | ALSA idle wake | M | Medium | Medium |
| OPT-4 | Resampler memmove | H | Medium | Low |
| OPT-5 | Deinterleave alloc | M | Medium | Low |
| OPT-6 | Opus buffer alloc | M | Low | Low |
| OPT-7 | Decode pre-alloc | M | Low | Low |
| OPT-8 | Reuse reqwest Client | M | Low | Low |
| OPT-9 | Seek without full decode | H | High | Medium |
| OPT-10 | Rate limiter prune | L | Low | Low |
| OPT-13 | Incremental decode | H | High | Medium |
| OPT-14 | Ring buffer | M | Medium | Low |
| OPT-15 | Track cache size | M | Low | Low |
| OPT-16 | Debounce manifest | M | Low | Low |
| OPT-18 | Recovery poll interval | L | Low | Low |
| OPT-19 | Producer notify | M | Medium | Low |
| OPT-21 | Default features | M | Low | Low |
| OPT-22 | Tokio features | M | Low | Low |

---

## Recommended first steps

1. **OPT-8** (reuse reqwest Client) — trivial, immediate network/CPU win
2. **OPT-15** (track cache size) — trivial, reduces eviction syscalls
3. **OPT-16** (debounce manifest) — trivial, reduces disk writes
4. **OPT-2** (ALSA chunk size) — one-constant change, reduces syscalls
5. **OPT-4** (resampler memmove) — high impact on long tracks, medium effort
6. **OPT-13** (incremental decode) — highest memory impact, but significant refactor
7. **OPT-3** (ALSA idle wake) — best for battery/idle, medium effort
