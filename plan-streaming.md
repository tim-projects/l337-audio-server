# plan-streaming.md — MKV demuxer wedge fix via a streaming `MediaSource`

**Problem:** Symphonia's MKV/WebM reader snapshots stream length at `try_new()` (via `seek(SeekFrom::End(0))`) and freezes `total_len`. When an element/cross-cluster boundary lands past that snapshot in a still-downloading file, `DecodeError("past master element")` fires permanently for that reader instance — there is no resync. Today l337 decodes from a growing on-disk file via `FileSource`, and the `rebuild_due()`/`wedge_action()`/`reprobe_format()` replay loop tries to re-probe and re-create the reader as bytes land. Each rebuild re-freezes `total_len` near the download frontier, so on a slow trickle the wedge recurs; rate-limiting it trades off between spin loops (the reverted `last_rebuild_at = None` fix) and long stalls waiting for growth that may never come. Regression test `test_mkv_wedge_streaming` (added 2026-11-12) confirms: `/player/play` returns 200 but `position_sec` never advances.

**Fix direction (Option A):** stop decoding a mutating file that must be re-probed/rebuilt as it grows. Feed HTTP bytes into a single shared, append-only byte source and give Symphonia one long-lived `MediaSource` over it. Reads at the frontier block until bytes arrive (or true EOF/error/cancel); `byte_len()` reports a stable final length; one reader instance lives for the stream — no reopen, no `replay_skip`, no `rebuild_due` cadence tuning.

**Device coverage:** RAM-backed streaming is the default (lowest latency, no disk wear). A file-backed cache mode is an optional second implementation for large/lossless tracks and devices with tight RAM, sharing the same reader contract.

---

## 1. Cherry-picked infra per component

### 1.1 Shared state — `StreamingByteSource` (abstract)
Single shared object owned by the download task, cloned into reader handles. Fields:

| Field | Meaning |
|-------|---------|
| `downloaded: AtomicU64` | monotonic count of committed bytes (writer advances as chunks land) |
| `total_len: Option<u64>` | HTTP `Content-Length`; final, stable logical end; `None` if chunked/unknown |
| `status: Atomic<Downloading \| Complete \| Failed \| Error>` | transfer lifecycle |
| `error: Option<String>` | failure reason when `Failed`/`Error` |
| `cancel: AtomicBool` | user stop / track change — must wake all blocked readers |
| notifier: `Condvar` (std) or `tokio::sync::Notify`-bridge | wakes blocked readers on append / finish / fail / cancel |

Writer contract (replaces body of `stream_http_to_file`): resolve headers first → learn `Content-Length` → construct source → spawn body-pump appending chunks → on finish set `Complete`; on HTTP/IO error set `Failed`/`Error` + message; respect `cancel` by aborting and waking readers.

### 1.2 Reader — `StreamingMediaSource: Read + Seek + MediaSource`
One handle per decoder/probe, each with its own `pos: u64`; all share the state above.

**`Read::read`** (blocking, stalls not EOF):
- If `buf` empty → `Ok(0)`.
- `available = downloaded` (or file metadata len when Complete + file-backed).
- If `available > pos`: copy `[pos, min(pos+len, available))`; advance `pos`; return count.
- If `available <= pos`:
  - `status == Failed/Error` → return `io::ErrorKind::BrokenPipe` with message (never clean EOF — prevents silent truncation looking like a short file).
  - `status == Complete && pos >= available` → return `Ok(0)` real EOF.
  - `status == Downloading && total_len known && available >= total_len && pos >= total_len` → `Ok(0)`.
  - Else (probe mode off) → block on notifier until append/finish/fail/cancel/deadline (stall timeout, default 30s → `TimedOut`).
  - Probe mode on (no waiting) → return `UnexpectedEof` "past what has arrived" immediately so the probe can fail fast instead of waiting for a tail that may never arrive.
- `cancel` true → wake and return error (so `spawn_blocking` decode exits promptly on stop/skip).

**`Seek`**:
- `Start(n)` / `Current(d)` → set `pos` (clamps at 0; forward seeks past write head are legal — the next `read` blocks until bytes arrive).
- `End(n)` → depends on `advertise_len`/`whole_file_end`/`total` (see ProbeMode below).

**`MediaSource`**:
- `is_seekable() -> true` (backward/forward below write head are `lseek`-cheap on file-backed; in-memory likewise; forward-past-head blocks honestly on read).
- `byte_len() -> Option<u64>`: **`total_len` when advertised**, else `None`. Never "current fill" — that points Matroska demuxers at the middle of the download.

### 1.3 Probe modes (cherry-picked from koan `PartialFileSource`)
The demuxer probe and the actual decode have opposite needs:

| Mode | `byte_len()` advertised | `SeekFrom::End(0)` | Read waits at write head? | Use |
|------|------------------------|--------------------|---------------------------|-----|
| `Full` | `total_len` | whole file | Yes | Decode of a stream whose container describes itself from prefix (MKV/WebM Opus header + early clusters). |
| `Lengthless` | `None` | write head (`available`) | No | Probe of containers that must not go hunting for a missing tail (Ogg/Opus last page, MP4 `moov`-at-end) — probe answers "not from what arrived" immediately; caller falls back to whole-file download. |
| `LengthlessWholeEnd` | `None` | whole file (`total`) | No | Ogg: no length (don't bisect into missing bytes), but end = whole file so duration isn't reported as 0. |

Rationale (from koan tests): answering `SeekFrom::End` with the *advertised* total when `byte_len()` is `None` sends FLAC/Matroska bisecting into bytes not yet on disk — every probe waits out the stall limit. Answering with the *write head* when `byte_len()` is `Some` reports a wrong end (mid-download). Length and end must agree.

### 1.4 Backend implementations (two, shared reader contract)
**A. In-memory (default — RAM):** shared `Arc<Vec<u8>>` (or chunked `Vec<Vec<u8>>`) + state. Download appends; readers slice directly. Simple, zero-copy reads, lowest TTFS. Bounded concern for very long/lossless tracks — document memory ceiling (e.g. warn/fallback above a configurable byte cap).
**B. File-backed (optional — disk cache):** `std::fs::File` on `slot/current` gated by `downloaded` counter (koan `PartialFileSource`). Downloader tees chunks to the file and advances `counter`; readers `read`/`seek` from the file up to `available`. Keeps RSS flat, preserves existing slot/cache/persist-slot layout (`storage.get_active_slot_path`, `persist_slot`). Choose per-track or per-config (small → RAM, large → file, or user preference).

### 1.5 Error semantics decision
- **Mid-stream download failure → `BrokenPipe` error (koan)**, not silent EOF (zuno). Rationale: clean EOF mid-track looks like a truncated song and the queue advances; a real error surfaces so the caller can fallback/report. Zuno's "finish buffered frames then end early" is acceptable only where graceful truncation is explicitly desired — not our default.

### 1.6 Symphonia compatibility (evidence base)
- Symphonia **PR #340** (merged 2025): FLAC/ISOMP4/MKV demuxers now prefer `byte_len()` over `seek(End)` for total length — avoids blocking/full-download for length on streaming/remote sources. Makes `byte_len() == Content-Length` the correct contract.
- **Discussion #302**: duration/metadata requires real `byte_len()` (not `None`).
- `MediaSourceStream` read-ahead buffer (1–32 KiB, grows exponentially) reduces syscall overhead on our reader; sequential playback stays within it.
- Recent MKV fixes (commit `8abf8fe`, issue #504 — `UnexpectedEof` on truncated binary element) align with why premature EOF / mid-download length snapshots break Matroska: our blocking-read + stable-length source avoids both.

---

## 2. Integration points in `src/player/engine.rs`

| Location | Current | Planned (Option A) |
|----------|---------|--------------------|
| `start_streaming_playback` (~928) | `stream_http_to_file` to `slot/current`; `decode_path: PathBuf` to decode | Create `StreamingByteSource` from reqwest response headers (`Content-Length`); spawn pump that appends to source (and tees to file in file-backed mode); pass `Arc<dyn StreamingByteSource>` (or enum `Mem/File`) into decode instead of `PathBuf`. |
| `streaming_decode_sync` (~3327) | Opens `FileSource{file}` on path; probe loop sleeps/retries on format failure; `reprobe_format` + `replay_skip` + wedge rebuild loops in decode match arms | Construct `MediaSourceStream` from `Box::new(StreamingMediaSource::new(source, mode))` for probe **and** decode — same reader instance. Probe uses `Lengthless`/`LengthlessWholeEnd` (fail fast); decode uses `Full` once probe succeeds. Drop `reprobe_format`/`replay_skip`/`wedge_action` rebuild loops for the growing-partial case. |
| `IoError` / `wait_for_progress` (~3159, ~3482, ~3736) | Wait on file growth via `wait_for_progress` (200 ms cycles, 3000-cycle tolerance) | Largely unnecessary: reader already blocks at frontier. Real download failure surfaces as `source.error`/`BrokenPipe`; decode loop propagates and stops cleanly. |
| Probe budget (`STREAM_PROBE_BUDGET` 35s) | Timeout → whole-file fallback | Preserve: if probe cannot complete within budget (container needs tail/index not yet in prefix, or `Lengthless` probe refuses to wait), cancel streaming and fall back to whole-file download — exactly current fallback behavior. |
| `stop()` (~825) | `cancel.store(true)`; abort download; await decode with 5s bound | Same, plus `cancel` must wake blocked `read()` calls so the `spawn_blocking` decode producer exits without waiting for stall timeout. |
| Fallback path (~1076) | `stream_http_to_file` whole file, then `load_and_play("current")` | Unchanged — whole-file download to slot file, then `FileSource`-based `decode_file_incremental_blocking` (existing, works for complete files). |

---

## 3. Edge cases & constraints (with mitigations)

| Edge | Mitigation |
|------|------------|
| Unknown / chunked `Content-Length` | `byte_len() -> None`; demuxers that require `Seek(End)` at open (Matroska tail cues) cannot be satisfied mid-stream → detect via `Lengthless` probe failure within budget → whole-file fallback. |
| Container needs tail/index not yet available (progressive MP4 `moov` at end) | `STREAM_PROBE_BUDGET` expires → fallback to whole-file download (current behavior, preserved). |
| Download stalls permanently | Stall timeout (default 30s) → `TimedOut` error → decode stops / fallback offered. |
| Download fails mid-track | `BrokenPipe` error (not EOF) → decode fails cleanly; caller may fallback or report. |
| User stop / skip mid-stream | `cancel` wakes blocked readers; `stop()` awaits decode with bound. |
| Backward seek into already-downloaded bytes | `lseek`/memory slice — cheap, no re-download. |
| Forward seek past write head | Legal; next `read` blocks until bytes arrive (callers/clients should clamp seek targets to `seekable_ms`/`downloaded`). |
| Memory pressure (RAM mode) | Configurable byte cap; above it use file-backed mode or fall back to whole-file download. |
| File-backed + rename at end | koan pattern: open descriptor survives `.part` → final cache path rename mid-playback; reader unaffected (reads gated by `downloaded` counter, not file metadata at open). |

---

## 4. Why not the alternatives (research summary)

- **Keep `rebuild_due`/`wedge_action`/`reprobe_format` replay** (current): brittle by construction — each rebuild re-freezes `total_len` near a moving frontier; spin-loop vs stall tradeoff is unresolvable by tuning intervals. Reverted fix (commit `bb205e2`) proved this.
- **Whole-file download before decode for everything**: destroys TTFS (~180s on 32KB/s link per changelog) and defeats the G3 streaming-default requirement. Keep only as fallback.
- **MPD model (FFmpeg/libavformat + `ThreadInputStream` ring buffer → `AVIOContext`)**: fundamentally superior for this failure mode but would mean replacing Symphonia's MKV reader with FFmpeg — out of scope for a targeted fix; Option A achieves the same stream semantics within Symphonia.
- **zuno `BufferReader` (polling, silent-EOF-on-fail)**: good precedent but simpler than needed; koan's condvar + `BrokenPipe` + `ProbeMode` triad is the richer, more correct cherry-pick.

---

## 5. Acceptance criteria (test-driven)

1. `tests/endpoints.rs::test_mkv_wedge_streaming` passes: throttled (32 KB/s) WebM/Opus via `googlevideo.com/videoplayback` URL → `position_sec` advances beyond 0 during streaming playback (currently fails).
2. Duration match: `position_sec` at `stopped` == ffprobe duration within ±2s for 5s/30s/120s WebM/Opus fixtures (per `scripts/match_mp3_duration.sh` pattern).
3. No spin loop: error cycles do not explode when download is complete but reader is wedged (reproduce the 40ms dozens-of-rebuilds scenario — must not recur).
4. Seek within downloaded range works during streaming; seeking past frontier blocks or clamps cleanly.
5. Mid-stream download failure → decode error, not silent early `stopped`.
6. RAM mode default; file-backed mode optional and memory-flat for long tracks.

---

## 6. Implementation order (suggested)

1. Add `StreamingByteSource` shared state + `StreamingMediaSource` reader + `ProbeMode` to `src/player/engine.rs` (or a new `src/player/streaming_source.rs`).
2. Wire into `start_streaming_playback`: headers → source → pump; pass source to decode.
3. Replace `FileSource` in `streaming_decode_sync` with `StreamingMediaSource`; probe with `Lengthless`, decode with `Full`; remove wedge rebuild/replay loops for this path.
4. Add file-backed backend variant (`FileByteSource` gated by `downloaded`) behind a feature or config toggle; RAM remains default.
5. Update `stop()` to ensure cancel wakes blocked readers.
6. Run `test_mkv_wedge_streaming` + `match_mp3_duration.sh` 5s/30s/120s; add CHANGELOG + bugs.md entries; commit.

*(No code changes made in this analysis pass — plan only.)*
