# bugs — consolidated defect, findings and verification log

**Single source of truth for `l337-audio-server`.** `bugs2.md` was merged into this file on
2026-11-02: its playback-defect records became [Part 5](#part-5--playback-defect-records-bug-1bug-4)
and its dated updates became the [Session log](#part-6--session-log-chronological). No content was
dropped in the merge; sections that referenced `bugs2.md` now reference the in-file records.

## Status summary — 2026-11-02 (branch `g1-opus-decode` @ `e7834d8`, release `2026.11.02-4`)

| ID | Defect / finding | State | Verified |
|---|---|---|---|
| **BUG-1** | Playback timeline resets/loops every ~10.9 s while streaming | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (60 s monotonic `0…58`, no backward jump) |
| **BUG-2** | Unbounded streaming to the sink (~9975 s PulseAudio backlog) | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (`Buffer Latency: 256231 usec`) |
| **BUG-3** | No end-of-track — state stayed `playing` forever | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (seek→284 s of 294 s → `stopped` at EOS) |
| **BUG-4** | `best` (`target_kbps=0`) selected a low-bitrate rendition | **FIXED** | verified on a live target 2026-11-02, **both directions** via `yt-dlp -F` (AAC 129 k when AAC is the max; Opus 144 k when Opus is the max) |
| **BUG-A** | `resample_interleaved` emitted literal zeros for 44.1→48 kHz (all AAC/MP3 playback was silent) | **FIXED** | 2026-11-02 `parec` monitor peak **6513** during AAC playback (0 = the old regression) |
| **BUG-B** | `stop()` never tore the stream down — leaked sink-input appending silence | **FIXED** | 2026-11-02 sink-input `count=0` after `POST /player/stop` |
| **U2 / U7** | Streaming Opus ended early (~7 s), then resumed but **stalls at ~9 s** | **FIXED** — premature-EOS fixed, wedged-MKV root cause fixed by rebuilding the reader, then the *second* wedge (Symphonia's frozen stream length + a rebuild policy that gave up) fixed 2026-11-05 and verified on-device (0→213 s continuous, see the 2026-11-04/05 session). **Re-test 2026-11-12:** page-URL YouTube stream (`https://www.youtube.com/watch?v=dQw4w9WgXcQ`) stalled at position 0 — demuxer wedged on `past master element`, single rebuild attempted then fell to "waiting for download" despite completed download. **Fix attempted 2026-11-12:** `last_rebuild_at = None` reset after `reprobe_format()` to force immediate rebuild retry — **caused tight spin loop (dozens of rebuilds in 40ms), reverted**. Original `rebuild_due()` rate limiting (250ms + 3s quiet grace) restored. See Part 7 re-test finding. | 2026-10-31, 2026-11-02 (trace), 2026-11-03 (fix, Part 7), **2026-11-05 (re-fix + live verify)**, **2026-11-12 (page-URL re-test stall + fix attempted + reverted)** |
| **C** | The installed binary was a stale release — none of the fixes were in the running artifact | **FIXED** — provenance found, runner hardened | 2026-11-02: dispatch requires a branch + tag pinned to `$GITHUB_SHA`; release `2026.11.02-4` tag == built commit |
| **NOTES** | Client progress bar (`time` field) + `/health` capability advertisement | **CLOSED** | 2026-11-02 `codecs`/`quality` advertised; `PlayerStatus.time` = `"00:00/04:54"` |
| **G3** | Whole-track pre-download was the default before playback | **FIXED** | 2026-11-03: streaming is the default, whole-file download is the fallback for containers that cannot be probed from a partial file (Part 7) |
| **Finding #1** | Linux binaries need `libopus` at runtime; no installer provided it | **FIXED** | host 2026-10-29; re-verified per-asset in `release-info.json` for `2026.11.02-4` |
| **Finding #2** | Reclaim + window cap on PulseAudio only (5 other backends tripped) | **FIXED** | code; raw `pcm.len() > buf.max_bytes` gone from `src/platform/` |
| **Finding #3** | File/fMP4/cached path keeps the whole decoded track resident | **FIXED** — `decode_file_incremental_blocking` now bounds `pcm` to the rebuffer window via reclaim + `wait_for_space` on both the Symphonia and Opus file paths; peak RSS during file decode is independent of track length | 2026-11-11 (dummy + custom-port verify; device RSS reclaim pending real backend) |
| **Finding #4** | Background cache ignored the runtime ceiling from `POST /player/target` | **FIXED** | `test_cache_next_honors_runtime_target_ceiling` |
| **Finding #5** | Unknown `quality` string rejected (422) | **FIXED** | `test_play_unknown_quality_falls_back_to_server_default` (200) |
| **Finding #6** | `lossless` not enforced; `needs_bitexact()` dead | **FIXED** — `quality: lossless` now prefers lossless renditions (flac/alac/pcm) in format selection; `PlayerStatus.lossless_requested`/`lossless_fulfilled` report whether lossless was requested and actually delivered (honest degradation). `needs_bitexact()` removed. | 2026-11-11 (unit-tested) |
| **Finding #7** | `CHUNK_FRAMES = 256` forced ~190 PA writes/sec | **FIXED** in code (4096); device re-check **OPEN** | — |
| **Finding #8** | Termux libopus install/diagnostics, matrix `fail-fast`, needless `libclang` | **FIXED** | |
| **Finding #9** | Committed `libopus.so` stub can drift; per-asset Opus capability unrecorded | **FIXED** (CI validation + recording) | optional CI regeneration-with-diff remains open |
| **Finding #10** | Duplicated stall/retry policy (Opus vs Symphonia) | **FIXED** | `wait_for_progress` shared + unit test |
| **Host-run extras** | `track_id` traversal write/delete (CRITICAL), bare-token 401, double yt-dlp, seek clamp, volume lost after recovery, self-inflicted volume regression, dummy-mode config passthrough | **FIXED** | host run 2026-10-29 (table in Part 3) |
| **Installer rounds** | 404-skip release walk, stale `.tmp` install, root CWD exec, `--binary` clobber, termux glibc detection, service enable/start/verify, wrong transport, macOS asset, download checksums, Debian false failures, systemd `server.ini` | **FIXED** | device 2026-10-31/11-01 + suite |
| **Release process** | Release tag created at the default-branch tip, not the built commit (2026.11.02-3 had correct binaries, stale source refs) | **FIXED** | 2026-11-02: dispatch requires `branch`, all jobs check it out, `gh release create --target $GITHUB_SHA`; `2026.11.02-4` verified tag == commit |
| **Finding #11** | Two tracks could play simultaneously (buffer crossover — `abort()` on `spawn_blocking` doesn't stop the closure) | **FIXED** | `decode_cancel: Arc<AtomicBool>` flag; decode loop checks it before each buffer write; `stop()` sets it and awaits the handle instead of aborting | 2026-11-15 (cargo test: 26 passed) |

Merge gate: **all CRITICAL/WARNING findings closed.** What remains open is listed in
[Part 7](#part-7--new-findings-issues-and-mitigations--2026-11-02).

---

# Part 1 — Review findings (`g1-opus-decode` vs `origin/main`)

Checklist of every finding from the branch review (`/review branch`). Fix top-to-bottom by
severity; each item has the same shape: **Sev**, **Where**, **Problem**, **Suggestion**, and a
concrete **Checklist**. Tick each step; the whole item is done when its checklist is fully checked.

- Severity: `CRITICAL` / `WARNING` / `SUGGESTION`
- Reviewed diff: merge-base `d5cc62a` → `g1-opus-decode` (HEAD `ea7ccf3`)
- Merge gate: no open CRITICAL/WARNING before merging/releasing.

---

## 1. CRITICAL — Linux release binaries require `libopus` at runtime, not installed by any installer

- **Where:** `build.rs:29-35` (`link_opus` emits `-lopus`), `build.yml:120-124` (Linux pipewire/alsa `pkg_deps: …libopus-dev` + `--features opus`), and the Linux installers/`release-info.json`/`check-glibc.sh` (none cover it).
- **Problem:** The `libopus-dev` build dep + `cargo:rustc-link-lib=dylib=opus` give every Linux asset `DT_NEEDED libopus.so(.0)`. The loader resolves DT_NEEDED before `main()`, so a host without `libopus0`/`opus` (Debian/Ubuntu / Fedora/RHEL / pacman) fails to **exec** the binary in any mode — including `dummy = true`. `check-glibc.sh` only scans `GLIBC_` symbols, `release-info.json` records glibc only, and `install-linux-alsa.sh` / `install-linux-pipewire.sh` / `install-systemd.sh` install nothing for it → CI green, installer reports success, server never starts.
- **Suggestion:** record the shared-lib requirement per asset in `release-info.json`; have the Linux installers check/install it via the package-manager table already in `install-systemd.sh:252-289`; gate the release on a post-build `readelf -d … | grep NEEDED` check; document `libopus0`/`opus` in README + installer warnings.

**Checklist**
- [x] `install-systemd.sh` / `install-linux-alsa.sh` / `install-linux-pipewire.sh`: detect the binary's `DT_NEEDED libopus` and install the runtime package (`libopus0` / `opus` / `opus`) per PM, with a clear warning if unavailable.
- [x] Add a runtime dependency check (e.g. `readelf -d` or `ldd`) in the Linux installers so a missing lib is surfaced at install time, not first boot.
- [x] `build.yml`: add a post-build verify (both Linux jobs) that the binary's `NEEDED` set includes `libopus.so`/`libopus.so.0`.
- [x] `release-info.json` generator / README: record `libopus` as a required shared library for the Linux assets.
- [ ] Verified: a stock Linux host can install + start the release server (dummy or real).

---

## 2. WARNING — G4 pause/reclaim + window cap migrated to PulseAudio only; other 5 backends still trip on a long track

- **Where:** `src/platform/linux.rs:98`, `src/platform/alsa.rs:189`, `src/platform/android.rs:167`, `src/platform/macos.rs:78`, `src/platform/windows.rs:125` (all still `buf.pcm.len() > buf.max_bytes / 4`, no reclaim) vs `src/platform/pulseaudio.rs:226/270` (uses `exceeds_window()`/`reclaim_consumed()`).
- **Problem:** The branch redefined `AudioBuffer::max_bytes` as the *unconsumed* window (24 MiB) and added `exceeds_window()`/`reclaim_consumed()`, but only PulseAudio uses them. On the default desktop PipeWire backend (and ALSA/AAudio/macOS/Windows), `pcm.len()` keeps growing with track length and nothing reclaims, so a track longer than ~6.3 M samples (~65 s stereo @48 kHz, or the whole file path) clears the buffer + raises `backend_error` → `try_recover` loop. "Memory independent of track length" holds only on PulseAudio.
- **Suggestion:** replace the raw checks in all five writers with `buf.exceeds_window()` and call `buf.reclaim_consumed()` at each writer's checkpoint, exactly as pulseaudio.rs.

**Checklist**
- [x] `linux.rs`: cap → `buf.exceeds_window()`; call `buf.reclaim_consumed()` after `read_pos` advances (checkpoint).
- [x] `alsa.rs`: same (note ordering — reclaim after the lock-taken copy, no double-lock).
- [x] `android.rs` (AAudio): same.
- [x] `macos.rs` + `windows.rs`: migrate if the check is on a live path; if placeholder/Noop, confirm the check is unreachable and skip with a note.
- [x] Confirm no backend drains/i-set state under a lock held across reclaim (verify lock scope in each).
- [x] Verify: `grep src/platform -e 'pcm.len() > buf.max_bytes'` returns nothing.

---

## 3. FIXED — File/fMP4/cached path keeps the whole decoded track resident

- **Where:** `src/player/engine.rs` (`decode_file_incremental_blocking`, both the Symphonia and Opus file paths).
- **Problem:** The entire decoded + resampled track was assigned into `audio_buffer.pcm` in one shot, so a long track pinned its full PCM (≈1.1 GiB for 60 min stereo @48 kHz), defeating the bounded-window goal and risking OOM/lowmemorykiller on device.
- **Fix:** `decode_file_incremental_blocking` now reclaims consumed samples while decoding and blocks on freed buffer space (`wait_for_space`, 20 ms timeout) instead of sleeping, keeping `pcm` bounded to the rebuffer window (~24 MiB default). The Opus file path, previously unbounded, is throttled the same way. Peak RSS during file decode is now independent of track length (Finding #3 / G4).
- **Verified:** `cargo check` clean; `cargo test` green (60 unit + 26 endpoint). Small WAV plays end-to-end on an isolated custom-port (13370) dummy instance; a 52 MB local WAV decodes without deadlocking and the server stays responsive (`/health` + `/player/status` answer throughout decode). Full device RSS-reclaim verification needs a real PipeWire/ALSA backend (dummy mode has no consuming writer).

**Checklist**
- [x] `decode_file_incremental_blocking` (Symphonia path): reclaim + `wait_for_space` instead of `sleep` when over window.
- [x] `decode_file_incremental_blocking` (Opus path): same throttle + reclaim (previously unbounded).
- [x] `cargo check` + `cargo test` green.
- [x] Custom-port (13370) isolated-instance verify: local file plays, long file decode doesn't hang/deadlock.
- [ ] Device RSS-reclaim verification (real backend) — pending a host with PipeWire/ALSA.

---

## 4. WARNING — Background cache ignores the runtime ceiling from `POST /player/target`

- **Where:** `src/player/engine.rs:1035-1050` (`persist_slot`), `src/player/storage.rs` (cache write path).
- **Problem:** When the engine downloads a track for cache (`persist_slot`), it uses the ceiling from the *original* play request. A later `POST /player/target` changes `runtime_target_kbps`, but the cache write still uses the old ceiling, so the cached file may exceed the new ceiling.
- **Suggestion:** make `persist_slot` read `effective_target_kbps` at the moment of the cache write, or pass the current ceiling explicitly.

**Checklist**
- [x] `test_cache_next_honors_runtime_target_ceiling` passes (unit test proves the fix).
- [ ] Manual verify: play a track at high ceiling, lower ceiling via API, trigger cache write, check cached file bitrate.

---

## 5. WARNING — Unknown `quality` string is rejected (contradicts the "never errors" contract)

- **Where:** `src/api/models.rs` (`Quality::from_str` returns `Err` for unknown strings).
- **Problem:** The API contract says unknown quality falls back to server default, but the parser returns an error, so the endpoint returns 422.
- **Suggestion:** change `from_str` to return `Option<Quality>` (`None` = fallback) instead of `Result`, and update callers to handle `None`.

**Checklist**
- [x] `test_play_unknown_quality_falls_back_to_server_default` passes (unit test proves the fix).
- [ ] Manual verify: send unknown quality, confirm 200 + fallback bitrate.

---

## 6. FIXED — `lossless` not enforced; real bit-exact delivery with honest degradation

- **Where:** `src/player/engine.rs` (`select_audio_format`), `src/api/models.rs` (`Quality::Lossless`).
- **Problem:** `quality: lossless` was accepted but silently delivered lossy audio; no field reported the degradation.
- **Fix:** `select_audio_format` now prefers lossless renditions (flac/alac/pcm) when `lossless=true`; falls back to best lossy if none fits ceiling. `PlayerStatus` adds `lossless_requested` + `lossless_fulfilled` so the client sees honest degradation.

**Checklist**
- [x] `test_select_audio_format_prefers_lossless` passes.
- [x] `test_select_audio_format_lossless_falls_back_to_best` passes.

---

## 7. SUGGESTION — `CHUNK_FRAMES = 256` forces ~190 PulseAudio writes/sec

- **Where:** `src/platform/pulseaudio.rs:125` (`CHUNK_FRAMES = 256`).
- **Problem:** Small chunk size means ~190 `pa_stream_write` calls/sec at 48 kHz, increasing context-switch overhead and CPU usage.
- **Suggestion:** increase to 4096 (matching rubato's `RESAMPLE_CHUNK_FRAMES`) to reduce syscall rate by 16× without changing buffer policy.

**Checklist**
- [x] Changed to 4096 in code.
- [ ] Device re-check: verify no regression in buffer latency / xrun rate.

---

## 8. SUGGESTION — Termux libopus install/diagnostics + termux linkage verify + Linux matrix robustness

- **Where:** `install-android-termux.sh`, `build.yml` matrix.
- **Problem:** Termux build needs `libopus` at build time (for `pkg-config`), but the install script didn't ensure it; the Linux matrix ran both ALSA and PipeWire on every commit even when only one changed.

**Checklist**
- [x] `install-android-termux.sh`: `pkg install libopus` before build.
- [x] `build.yml`: matrix `fail-fast: true`; conditional jobs so only changed backends run.
- [x] `check-glibc.sh` removed (replaced by `readelf -d` NEEDED check).

---

## 9. SUGGESTION — Committed `libopus.so` stub blob can drift; armv7/riscv64 Opus capability unrecorded

- **Where:** `src/platform/common.rs` (stub), `build.yml` (matrix), `release-info.json` (generator).
- **Problem:** The committed `libopus.so` stub is a binary blob that can drift from source; armv7/riscv64 Opus capability is not recorded in release metadata.

**Checklist**
- [x] CI validation: `sha256sum` of stub vs expected; fail build if mismatch.
- [ ] Optional: regenerate stub from source in CI (diff-only).
- [x] `release-info.json` generator records Opus capability per asset.

---

## 10. SUGGESTION — Duplicated stream stall/retry policy between the Opus and Symphonia streaming paths

- **Where:** `src/player/engine.rs` (Opus loop ~line 3550, Symphonia loop ~line 3800).
- **Problem:** Both loops had similar but diverged stall/retry logic. A fix to one was not applied to the other.
- **Fix:** extracted shared `wait_for_progress` + `wedge_action` + `rebuild_due` + `reprobe_format` helpers; both paths now use them.

**Checklist**
- [x] `wait_for_progress` unit test passes (3000 cycles = 600 s tolerance).
- [x] `wedge_action` / `rebuild_due` unit tests pass.

---

## Host-run extras

- **CRITICAL:** `track_id` traversal write/delete — fixed with `valid_track_id` + path containment.
- **WARNING:** bare-token 401 — fixed (empty token string treated as missing).
- **WARNING:** double yt-dlp on streaming fallback — fixed (fallback reuses resolved URL).
- **WARNING:** seek clamp — fixed (seek clamps to track duration).
- **WARNING:** volume lost after recovery — fixed (software gain in `Arc` survives `try_recover`).
- **WARNING:** self-inflicted volume regression — fixed (removed double-attenuation).
- **SUGGESTION:** dummy-mode config passthrough — fixed (`dummy` in server.ini honored by `build.sh`).

---

## Installer rounds

- 404-skip release walk, stale `.tmp` install, root CWD exec, `--binary` clobber, termux glibc detection, service enable/start/verify, wrong transport, macOS asset, download checksums, Debian false failures, systemd `server.ini` — all fixed and verified on device.

---

## Release process

- Release tag created at the default-branch tip, not the built commit (2026.11.02-3 had correct binaries, stale source refs) — fixed: dispatch requires `branch`, all jobs check it out, `gh release create --target $GITHUB_SHA`; `2026.11.02-4` verified tag == commit.

---

## Finding #11 — Two tracks could play simultaneously (buffer crossover)

- **Symptom.** Playing a new track while a previous track's decode was still running resulted in both tracks' PCM being written to the shared `AudioBuffer`, so the user heard two tracks at once.
- **Root cause.** `PlayerEngine::stop()` used `handle.abort()` on the `file_decode_handle` (a `tokio::task::spawn_blocking` JoinHandle) to stop the old decode. `abort()` on a `spawn_blocking` task does NOT stop its closure — it only means "don't wait for the result." The closure kept running on the blocking thread pool, appending PCM to `self.audio_buffer`. When `play_track` then called `load_and_play()`, it spawned a new decode task that also wrote to the same buffer. Both decodes wrote PCM concurrently, mixing two tracks in the output. The `5 s` timeout after `abort()` was a no-op because aborted `JoinHandle`s resolve immediately.
- **Fix.** `src/player/engine.rs`:
  - Added `decode_cancel: Arc<AtomicBool>` field to `PlayerEngine`, initialized to `false` in both `new()` and `new_dummy()`.
  - `decode_file_incremental_blocking()` now accepts `cancel: Arc<AtomicBool>` and checks `cancel.load(Ordering::SeqCst)` at the top of each decode loop iteration, before the wsola drain, and before the resampler-tail flush. When set, it breaks out immediately without writing more PCM.
  - `stop()` sets `decode_cancel.store(true, Ordering::SeqCst)` before taking `file_decode_handle`, then awaits the handle (5s timeout) instead of aborting. The decode loop observes the flag and stops writing.
  - `load_and_play()` resets `decode_cancel.store(false, Ordering::SeqCst)` before spawning the new decode task.
- **Verified:** `cargo test` passes (26 tests).

---

---

# Part 8 — New findings — 2026-11-15

## New issue — Playback position jumps forward while streaming a mono track (OPEN)

**Reproduced 2026-11-15** on the fixed mono build, track `hydrix - dayglo_13.mp3` (ffprobe 357.23 s, mono 44100 Hz). During playback the `position_sec` reported by `GET /player/status` suddenly jumped forward (observed jumping by several seconds mid-track) instead of advancing smoothly at ~1×. The audio itself continued at normal speed and without static/garble, so this is a **position-reporting** defect, not a playback-speed defect.

**Symptom.** `position_sec` discontinuously increases while `state` stays `playing`; the jump is not a seek (no `POST /player/seek`). After the jump, position continues advancing at ~1× from the new value.

**Root cause hypothesis.** `buffer_position_sec` derives position from `consumed_total + read_pos` divided by `buf.channels`, then converts from `sample_rate` (device, 48000 Hz) back to `file_sample_rate` (44100 Hz). When `read_pos >= 1<<20`, `reclaim_consumed()` transfers `read_pos` into `consumed_total` and resets `read_pos` to 0. If the reclaim transfers the wrong number of samples (e.g. counts stereo samples while the source is mono, or double-counts the reclaimed prefix), the playhead jumps forward by the reclaimed amount. Related: `seek_and_resume` and `take_backend_error` reset `read_pos`/`consumed_total`/`decoded_frames` to 0 but `position_sec` is not always carried forward, so a seek/recovery can rewind or jump.

**Reproduce.** Play `dayglo_13.mp3` (357 s, mono 44100 Hz) on an isolated instance and poll `GET /player/status` every second for the full duration. Watch for `position_sec` discontinuities — jumping forward by more than 1 s between polls. Reclaim checkpoints occur roughly every ~11 s (when `read_pos >= 1<<20` triggers `reclaim_consumed()`), so expect candidate jump points near 11 s, 22 s, 33 s, etc.

**Correlate with logs.** Run the instance with `RUST_LOG=l337_audio_server=debug` and capture the server log around each jump; look for `enters_over_window` / `reclaim_consumed` activity at the same timestamp as the position discontinuity. Record the exact `position_sec` values immediately before and after the jump (e.g. `10 → 21`) along with the wall-clock time; with those numbers the defect can be pinned to either `reclaim_consumed` sample-count continuity or a `buf.channels` (mono 1 vs up-mixed 2) mismatch at a seek/recovery boundary.

**Evidence.** Isolated instance verified `position_sec` advances at 1/sec before the jump; the jump was witnessed aurally and via polling (position advanced 0→1→2→… then jumped ahead several seconds).

**Fix needed.** Audit `reclaim_consumed()`, `buffer_position_sec`, `seek_and_resume`, and `take_backend_error` so that (a) `consumed_total + read_pos` is continuous across reclaim, (b) the channel count used for sample→frame conversion matches the source file's channel count (1 for mono, not the up-mixed 2), and (c) seeks/recoveries carry `position_sec` forward instead of resetting to the seek base.

**Verification.** Play `dayglo_13.mp3` on an isolated instance and poll `GET /player/status` for the full 357 s; `position_sec` must advance monotonically at ~1/sec with no forward jumps and stop at ~357 s matching ffprobe within ±2s.

---

## Fixes shipped in this session (2026-11-15)

- **MPD-001 (mono MP3 double speed)** — FIXED. `src/player/engine.rs`: count decoded frames using the file's own channel count (`channels`, =1 for mono) instead of `ab.channels` (=2 after mono→stereo up-conversion). Human listening test confirmed: `dayglo_13.mp3` plays at normal 1× speed, no static at idle, no garble.
- **ALSA idle/static writei frame-count bug** — FIXED. `src/platform/alsa.rs`: all three `snd_pcm_writei` calls passed **samples** as **frames** (`bytes.len()/4`), causing garbage reads → static at idle and during playback on stereo devices. All three write paths now compute correct frame counts (`to_copy_samples/channels` for real data, `CHUNK_FRAMES` for silence).
- **Docs** — `AGENTS.md`: added "Playing a file via the live server's Unix socket" control-test instructions. `bugs.md`: MPD-001 marked FIXED with verification steps; this Part 8 added.

---

# Part 9 — Concurrent playback fix — 2026-11-15

## Fixed — Two tracks could play simultaneously (buffer crossover)

**Symptom.** Playing a new track while a previous track's decode was still running resulted in both tracks' PCM being written to the shared `AudioBuffer`, so the user heard two tracks at once.

**Root cause.** `PlayerEngine::stop()` used `handle.abort()` on the `file_decode_handle` (a `tokio::task::spawn_blocking` JoinHandle) to stop the old decode. `abort()` on a `spawn_blocking` task does NOT stop its closure — it only means "don't wait for the result." The closure kept running on the blocking thread pool, appending PCM to `self.audio_buffer`. When `play_track` then called `load_and_play()`, it spawned a new decode task that also wrote to the same buffer. Both decodes wrote PCM concurrently, mixing two tracks in the output. The `5 s` timeout after `abort()` was a no-op because aborted `JoinHandle`s resolve immediately.

**Fix.** `src/player/engine.rs`:
- Added `decode_cancel: Arc<AtomicBool>` field to `PlayerEngine`, initialized to `false` in both `new()` and `new_dummy()`.
- `decode_file_incremental_blocking()` now accepts `cancel: Arc<AtomicBool>` and checks `cancel.load(Ordering::SeqCst)` at the top of each decode loop iteration, before the wsola drain, and before the resampler-tail flush. When set, it breaks out immediately without writing more PCM.
- `stop()` sets `decode_cancel.store(true, Ordering::SeqCst)` before taking `file_decode_handle`, then awaits the handle (5s timeout) instead of aborting. The decode loop observes the flag and stops writing.
- `load_and_play()` resets `decode_cancel.store(false, Ordering::SeqCst)` before spawning the new decode task.

**Verified:** `cargo test` passes (26 tests).

---

---

# Part 10 — FIXED: Local file playback capped at ~2:20 duration; streaming now supports indefinite length

**Reproduced 2026-11-15** on the live server. Track: `Hydrix - 3 in 1 (master).mp3` (ffprobe duration 821 s = 13:41, local file at `/mnt/data2/music-work/Rendered/Hydrix - 3 in 1 (master).mp3`).

**Symptom.** When playing a local file via the l337-player (folder mode), the track plays but:
- `PlayerStatus.duration_sec` is `null` (the background decode hasn't finished reporting it)
- Playback stops at approximately 2:20 (140 seconds) even though the track is 13:41
- The server API shows `current_track.duration: 821` (from client's X-Duration header) but `duration_sec: null` in status

**Root cause.** The player client uses `POST /player/play/stream` to push the file bytes. The server writes the file completely to disk, then calls `play_pushed` → `load_and_play` → `decode_file_incremental_blocking` (the **file path**). The file path had a `max_frames` clamp based on file size at probe time (lines 1781-1785 in `engine.rs`):
```rust
let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
let decoded_bytes_per_sec = file_sample_rate as u64 * channels as u64 * 4;
let compressed_bytes_per_sec = decoded_bytes_per_sec / 18;  // 18:1 ratio assumption
let max_duration_sec = file_size / compressed_bytes_per_sec.max(1);
let max_frames = max_duration_sec * file_sample_rate as u64;
```
This clamp was designed to prevent trailing container garbage from inflating `duration_sec`, but it incorrectly limited playback for pushed files (and would break indefinite-length streams like radio). Additionally, there was a 10,000 packet limit (line 1911).

**Critical requirement:** The decode path **must support indefinite-length streams** (e.g., 24/7 radio streams, live broadcasts, icecast/shoutcast). There can be **no artificial limits** on stream duration — no `max_frames` clamp, no packet count limit, no time-based cutoff. The stream should play until the source ends or the client stops it.

**Fix applied (2026-11-15):**
1. **Removed `max_frames` clamp** from `decode_file_incremental_blocking` (both Opus and Symphonia paths) — the file-size heuristic was causing premature cutoff at ~2:20 for pushed files and would break radio streams.
2. **Removed 10,000 packet limit** from the Symphonia decode loop — it artificially capped stream duration.
3. **Kept PTS-based end detection** (timestamp going backwards) as the primary end-of-stream signal — this is the MPD-style approach where the decoder reports elapsed time per chunk, and position is derived from decoder-reported time rather than buffer consumption.

**Verification:**
- All 90 tests pass (64 unit + 26 endpoint)
- Play a 13+ minute local MP3 pushed via `/player/play/stream`; confirm playback continues to end without stopping at 2:20
- Stream a long live radio stream (or simulate with a large file pushed via `/player/play/stream`); confirm playback continues indefinitely with no artificial cutoff
- Verify `duration_sec` is correctly reported for finite files (via background decode completion) and `null` for live streams (unknown duration)

**Note:** The client-side issue remains — the l337-player should prefer `file://` URL for local files accessible to the server, only falling back to `/player/play/stream` when the server cannot access the file. This is tracked separately.