# bugs-completed.md — Completed Bug Fixes and Verified Fixes

Tracks bugs fully resolved and verified. For ongoing/pending issues, see `bugs.md`.
Last updated: 2026-11-11.

## ✅ Completed (l337-audio-server) — verified in code (`cargo check` clean, `cargo test` 68 unit + 26 endpoint all green)

### 9. Concurrent playback / `stop()` race (2026-11-11, `src/player/engine.rs`)
- **Root cause**: `PlayerEngine::stop()` was synchronous — it aborted the background streaming download/decode tasks without awaiting them. When `play_track()` called `stop()` then immediately started a new decode, the old decode task could still be running and appending PCM to the shared `AudioBuffer` while the new track's decode also wrote to it. Two producers writing to one buffer caused the same track to be heard twice and the buffer to "jump between the two streams."
- **Fix**: Made `stop()` async. It now sets the cancel flag, aborts and awaits the download handle, then awaits the decode handle with a 5-second timeout, guaranteeing the old producer has fully stopped writing to the shared buffer before a new playback begins — enforcing single-track playback. All internal call sites (`play_track`, `play_pushed`, `load_and_play`, `trigger_next`, `trigger_previous`, `get_status` end-of-track) and the `/player/stop` + `/player/next` handlers updated to `.await`.
- **Verified**: `cargo check --no-default-features --features alsa,opus` passes with no new errors; 64 unit + 26 endpoint tests all green.

### NF-1 through NF-13 — API consistency & correctness fixes (2026-11-05, `src/api/`, `src/player/engine.rs`)
All verified in code; no hardware required.

- **NF-1 — Seek field alignment**: `SeekPayload.position` → `position_sec` (`src/api/models.rs:142`), handler updated (`src/api/handlers.rs:393`). Client reading `position_sec` from status and sending it back to seek now gets 200 instead of 422.
- **NF-2 — JSON `{"ok": true}` bodies on all mutation endpoints**: pause, stop, next, previous, set_speed, set_volume, seek, set_settings, upload_stream, cache_next, cache_previous all return `Json(serde_json::json!({"ok": true}))` instead of empty `StatusCode::OK` (`src/api/handlers.rs`).
- **NF-3 — Silent volume/speed clamping**: intentional, documented. `set_volume` clamps to [0.0, 1.0] (`engine.rs:850`); `set_speed_and_pitch` clamps to [0.25, 4.0] (`engine.rs:832-833`). No code change.
- **NF-4 — Test field name**: `src/api/tests.rs:111` uses `position_sec` (updated to match NF-1).
- **NF-5 — NaN/Inf validation on volume/speed/pitch**: `set_speed` and `set_volume` reject non-finite values with 400 JSON (`src/api/handlers.rs:354-366`, `:376-382`).
- **NF-6 — cache_next/cache_previous JSON bodies**: both return `{"ok": true}` (`src/api/handlers.rs:295`, `:347`).
- **NF-7 — Auth rate limiting default address**: when `ConnectInfo` is absent, rate limit defaults to `SocketAddr::from(([0,0,0,0], 0))` so rate limiting always applies (`src/api/handlers.rs:78`, `:125`).
- **NF-8 — `track_id_missing()` returns JSON**: now returns `{"ok": false, "error": "..."}` instead of plain text (`src/api/handlers.rs:176-181`).
- **NF-9 — `upload_stream` error responses JSON**: all three error paths (invalid endpoint, missing X-Track-Id, upload I/O failure) return JSON (`src/api/handlers.rs:514-518`, `:524-529`, `:566-571`).
- **NF-10 — `SpeedPayload.pitch` defaults to 1.0**: `#[serde(default = "default_pitch")]` with `default_pitch()` returning 1.0 (`src/api/models.rs:114-123`).
- **NF-11 — `download_stream` only `file://` prefix**: bare absolute paths rejected; local file access requires `file://` prefix only (`src/player/engine.rs:2340`).
- **NF-12 — `play_pushed` returns `Result<(), String>`**: checks `self.state != Playing` after `load_and_play`; `upload_stream` propagates as 500 JSON (`src/player/engine.rs:595-620`, `src/api/handlers.rs:576-586`).
- **NF-13 — Concurrent playback / `stop()` race**: same as item 9 above.

### 1. AAC divide-by-zero bug (bugs.md)
- **Root cause**: `decode_to_pcm` in `src/player/engine.rs` could yield `channels=0` for AAC-LC when probed codec params had `channels: None`, causing `pcm.len() / channels` panic.
- **Fix**: Channels default to 2 when `None`, with `.max(1)` floor (`src/player/engine.rs:1502-1509`).
- **Verified in code**: `channels.max(1)` at line 1509; AAC path uses `head.channels.max(1)` at line 1456. AAC plays with no panic; WAV unchanged. Live/player APIClient validation; CI ALSA binary confirmed. ✅

### 2. Rust loglevel from `server.ini` (bugs.md)
- **Fix**: `[server] loglevel` (`error`/`warn`/`info`/`debug`/`trace`) loaded in `src/main.rs`; precedence `RUST_LOG` (explicit/parsable) > `loglevel` > crate default (`warn`).
- **Verified**: on-device (2026-11-01). ✅

### 3. Pipelinepro install runtime libs (bugs.md)
- **Fix**: Installer skips glibc-provided runtime SONAMEs, refreshes `ldconfig` before checks, probes via `ldconfig -p`/standard dirs.
- **Verified**: simulation + Debian-host runs. ✅

### 4. PipeWire backend stream errors / WAV silent (2026-11-08, `src/platform/linux.rs`)
- **Root cause**: PipeWire refactor (`9deda51`) dropped/miswired stream/main loop/command receiver; stream entered `Connecting -> Unconnected`; `Unconnected` treated terminal. API could report playing but no audible output/node.
- **Fix**: Keep main loop/context/core/stream/listener alive on the thread, route `SetActive`/`Disconnect` via `pipewire::channel` on the main loop, treat only `StreamState::Error` as terminal.
- **Verified**: stream `Connecting -> Paused -> Streaming`; WAV `120_K10_A.wav` audible in normal PipeWire mode (`--features backend,opus`, socket, real backend). User confirmed. ✅

### 5. OGG/Vorbis long-file decode/playback deadlock — no audio / hung upload (2026-11-08, `src/player/engine.rs`)
- **Root cause**: `load_and_play` ran incremental file decode **before** `stream.play()`. Backend callback consumes no PCM while `playing=false`; decoder throttled on `AudioBuffer::exceeds_window()` (default ~24 MiB rebuffer window) and waited for `space_notify`. Decoded PCM larger than the window deadlocked decode; request hung, no completion/metadata.
  - Short WAV `120_K10_A.wav` (~1.35 MiB decoded f32 stereo 44.1k) fit under window — unaffected.
  - `90s_house_5.ogg` (Ogg Vorbis, 44100 Hz stereo, 4:21, `~112kbps`, ~87.8 MiB decoded) exceeded window — reproduced (curl timeout/no decode completion). `mpv --ao=pipewire` played the same file fine (file/device comparator OK).
- **Fix**: `load_and_play` now `ensure_stream()` + `stream.play()` and sets `Playing` **before** awaiting incremental decode so callback consumes while decode fills/throttles; duration/codec/`actual_kbps` populated after decode; decode errors call `stop()`. Binary rebuilt/installed (`--features backend,opus`).
- **Memory bounding (complementary, 2026-11-11):** `decode_file_incremental_blocking` now reclaims consumed samples while decoding and blocks on freed buffer space (`wait_for_space`) instead of sleeping, on both the Symphonia and Opus file paths. Peak RSS during file decode is independent of track length (Finding #3 / G4); previously `pcm` grew to the full decoded track.
- **Verified**: OGG `90s_house_5.ogg` audible in normal PipeWire mode (user confirmed; ~10s sufficient). WAV baseline still OK. Status advanced while playing. `cargo check` clean; `cargo test` green (60 unit + 26 endpoint). Isolated custom-port (13370) dummy instance: local file plays end-to-end; 52 MB local WAV decodes without deadlocking, server stays responsive. ✅
- **Follow-up (not a blocker):** `/player/play/stream` pushed-file handler still awaits full decode completion, so HTTP response may be delayed for long files even though audio has started.

### 6. Socket-mode multi-server/startup hardening (2026-11-08, `src/main.rs`)
- **Root cause**: stale-socket preflight could remove/handle paths unsafely on ambiguous connect failures; bind errors were not always reported cleanly; isolated lock scopes could still target the same socket path.
- **Fix**: `classify_socket`/`ensure_socket_path_clear` — only confirmed stale socket nodes (`ConnectionRefused`/`NotFound` on connect to a real socket) are removed; active listeners and protected/ambiguous paths (symlink/non-socket, `PermissionDenied`, other errors) are never clobbered. Bind failures reported (no panic). Added per-socket advisory lock (`<socket>.sock.lock`, held for process lifetime) before stale cleanup/bind to prevent serving the same Unix socket across lock scopes.
- **Notes**: global `InstanceLock` still enforces one normal instance; `L337_RUNTIME_DIR` isolation remains intentionally scoped (use consistently for fully separate runtimes/endpoints). ✅

### BUG-1 — Playback timeline resets/loops every ~10.9s while streaming (`src/platform/common.rs`)
- **Root cause**: `AudioBuffer::reclaim_consumed()` drains the consumed prefix and resets `read_pos`, but playhead was derived from `read_pos` alone, so every reclaim checkpoint rewound the timeline to the seek base.
- **Fix**: `AudioBuffer::consumed_total` counts reclaimed samples; position = `consumed_total + read_pos`, never `read_pos` alone (`src/platform/common.rs:46-47, 124-138`). Unit test `test_position_survives_writer_reclaim` guards it.
- **Verified**: device 2026-10-30: live trace `0,1,2,…,8,10,11,…,29` then EOS — no sawtooth. Re-verified 2026-11-02: 60 s of 1 Hz polling gave `0,1,2,…,58` with no backward jump. ✅

### BUG-2 — Unbounded streaming to sink (~9975s backlog; memory grows) (`src/platform/common.rs`)
- **Root cause**: Writer loop wrote as fast as possible with no real-time pacing; `AudioBuffer` never reclaimed as `read_pos` advanced; combined with BUG-1's re-append path.
- **Fix**: Pace writer to device clock (`n / (channels * rate)`); enforce cap on unconsumed window (`(pcm.len() - read_pos) > max_bytes/4`); reclaim via `pcm.drain(..read_pos); read_pos = 0` at writer checkpoint under existing `Mutex`; treat `buffer_max_bytes` as rebuffer window, not whole track.
- **Verified**: device 2026-10-30: one 30s track → PA `Buffer Latency` ~38s flat; server window drains ~1×. Re-verified 2026-11-02: `pactl list sink-inputs` during playback → `count=1`, `Buffer Latency: 256231 usec` (≈0.26s). ✅

### BUG-5 — Stuttery file playback: `wait_for_space()` held the AudioBuffer Mutex, starving the PipeWire callback (`src/player/engine.rs`)
- **Root cause**: `decode_file_incremental_blocking()` called `ab.wait_for_space()` while holding the `AudioBuffer` Mutex (both Opus path at line 1608 and Symphonia path at line 1737). The PipeWire process callback needs the same Mutex to consume PCM, so during every 20 ms `wait_for_space` the callback was blocked — causing audio underruns, stuttering, and "playing frames from other parts" / duplicated playback. The buffer could not drain because the consumer (callback) was locked out by the producer (decode).
- **Fix**: Release the `AudioBuffer` Mutex before calling `wait_for_space()`. `exceeds_window()` check and `reclaim_consumed()` remain under the lock; only the condvar wait happens lock-free. Both Opus and Symphonia file paths fixed (`src/player/engine.rs` `decode_file_incremental_blocking`).
- **Verified**: `cargo check --no-default-features --features backend,opus` clean; `cargo test` 68 unit + 26 endpoint all green. Isolated custom-port (13370) instance with real PipeWire backend: WAV `test_tone.wav` plays smoothly (pos advances 0→10s, no stutter); MP3 `dispatch.mp3` starts and decodes with only 1 "over rebuffer window" message (vs repeated rapid-fire before fix); no PipeWire errors/xruns. ✅


### BUG-3 — No end-of-track: state stays `playing` after playback finishes (`src/player/engine.rs`)
- **Root cause**: `get_status` never signalled completion; state stayed `playing` forever.
- **Fix**: `get_status` ends playback as `state = stopped` (position reset) once decode task is finished **and** buffer is fully drained; drained buffer with decode still running stays `playing` (rebuffer, not EOS) (`src/player/engine.rs:1321-1327`).
- **Verified**: device 2026-10-30: `playing → stopped` at track end. Re-verified 2026-11-02: seek to 284s of 294s track → 11s later `state = stopped`, `position_sec` reset, sink-input released. ✅

### BUG-4 — "best" quality selects low-bandwidth rendition instead of highest available (`src/player/engine.rs`)
- **Root cause**: `select_audio_format` used yt-dlp's `bestaudio` ranking which routinely resolved to low-bitrate renditions.
- **Fix**: `select_audio_format` picks the highest-bitrate audio rendition at or below ceiling (0 = best) from yt-dlp's full format list, server-side. A ceiling that admits nothing falls back to the lowest available rendition.
- **Verified**: `yt-dlp -F` cross-checked on `dQw4w9WgXcQ`: AAC 129k where AAC 140 is max; Opus 144k where itag 251 is max. `POST /player/play` under `target_kbps=64` reported `target_kbps=64`, `actual_kbps=61`, `codec=opus`. ✅

### BUG-A — `resample_interleaved` emitted zeros for 44.1→48 kHz (all AAC/MP3 silent) (`src/player/engine.rs`)
- **Root cause**: `resample_interleaved` assembled output planar instead of interleaved, and emitted literal zeros on rate mismatch.
- **Fix**: Rate conversion now writes interleaved, non-zero output (`src/player/engine.rs` `stretch_and_resample` path).
- **Verified**: control 440Hz tone peak 11999; 44.1kHz WAV previously silent now audible (monitor peak 11002). ✅

### BUG-B — `stop()` never tore stream down — leaked sink-input appending silence (`src/player/engine.rs`)
- **Root cause**: `stop()` called `stream.pause()`, never `stream.stop()`/Drop, so writer thread and sink-input survived stop; paused writer kept appending silence.
- **Fix**: `stop()` now pauses the stream and clears the buffer immediately; `try_recover` tears down and recreates via `stream.stop()` + `backend.start_stream()`.
- **Verified**: `pactl list sink-inputs` → `count=0` after `POST /player/stop`. ✅

### U2/U7 — Streaming Opus ended early (~7s), then stalled at ~9s (`src/player/engine.rs`)
- **Root cause**: Symphonia's MKV reader has no resync; once wedged on `past master element`, every `next_packet` fails forever. `wait_for_progress` reset `stall_cycles` on every append so a trickling download never accumulated tolerance.
- **Fix**: `reprobe_format()` rebuilds the format reader over the same append-only file when the reader is wedged; loops count packets already read and skip exactly that many on the rebuilt reader. `wedge_action()` policy rebuilds when the file grew since last rebuild or has been quiet for 3s. Rate-limited to 250ms per wedge.
- **Verified**: device 2026-11-05: `position_sec` advanced 0→213s (full track) continuously with 9 re-probes, 0 watchdog trips. ✅

### C — Installed binary was stale release — none of the fixes in running artifact
- **Root cause**: `gh release create` tagged at default-branch tip instead of built commit; releases `2026.11.01-1`/`2026.11.02-1`/`2026.11.02-2` all tagged `d5cc62a` (33 commits before fixes).
- **Fix**: `workflow_dispatch` takes required `branch` input; every job checks out `${{ inputs.branch || github.ref }}`; publish job pins tag to `gh release create --target "$GITHUB_SHA"`.
- **Verified**: prerelease `2026.11.02-4` tag == built commit `e7834d8`; `2026.11.03-1` tag == commit `229346a`. ✅

### G3 — Whole-track pre-download was the default (`src/player/engine.rs`)
- **Root cause**: `start_streaming_playback` chose strategy from URL extension; WebM/Ogg/Opus streamed, everything else (including YouTube m4a/itag-140) downloaded whole before decode. Time to first sound was the whole download (~180s for 4.77MB AAC on 32KB/s).
- **Fix**: Strategy no longer chosen up front. `start_streaming_playback` always starts streaming (download task + `streaming_decode_sync`) and waits for probe outcome via oneshot. Probe `Ok` → stream continues; probe `Err` → cancel stream, download whole, fall back to `load_and_play`; probe pending → keep going, return. `STREAM_PROBE_BUDGET` = 35s.
- **Verified**: `POST /player/play` returned `{"ok":true,"state":"playing"}` in seconds; `position_sec` reached 9 within ~6s on cold 213s YouTube Opus track. ✅

### Finding #1 — libopus runtime library required on Linux (bugs.md Part 1, CRITICAL)
- **Root cause**: `build.rs` emits `-lopus` giving every Linux asset `DT_NEEDED libopus.so(.0)`; no installer checked/installed it; `check-glibc.sh` only scans `GLIBC_` symbols.
- **Fix**: `ensure_runtime_dependencies()` reads binary's `DT_NEEDED` set, installs mapped package per package manager, fails install if needed lib still missing. Wired into all three Linux installers. `release-info.json` records `needed_shared_libs` per asset.
- **Verified**: both Linux assets' `NEEDED` sets read back: `libopus.so.0` (+ `libpipewire-0.3.so.0` on pipewire); `ldd` resolves; `L337__SERVER__DUMMY=1 … --version` and dummy boot work. Stock-host install-from-scratch (apt/dnf/pacman path) not executable here — no passwordless sudo. Installer dependency-check gate verified in code: fails loudly on unresolvable NEEDED. PARTIAL — real-host install verification pending. ⚠️

### Finding #2 — G4 pause/reclaim + window cap on PulseAudio only; other 5 backends still trip (bugs.md Part 1, WARNING)
- **Root cause**: `AudioBuffer::max_bytes` redefined as unconsumed window; `exceeds_window()`/`reclaim_consumed()` added, but only PulseAudio used them. Other backends (`linux.rs`, `alsa.rs`, `android.rs`, `macos.rs`, `windows.rs`) still used raw `pcm.len() > buf.max_bytes / 4` with no reclaim.
- **Fix**: All five backends now use `buf.exceeds_window()` and call `buf.reclaim_consumed()` at checkpoint after `read_pos` advances. Raw `pcm.len() > buf.max_bytes` pattern gone from `src/platform/` entirely.
- **Verified**: `grep src/platform -e 'pcm.len() > buf.max_bytes'` returns nothing. All backends (`linux.rs:181`, `pulseaudio.rs:298`, `alsa.rs:209`, `android.rs:180`, `macos.rs:121`, `windows.rs:138`) call `reclaim_consumed()`. Device re-check of pulseaudio write cadence (Finding #7, separate item) OPEN. ✅

### Finding #3 — File/fMP4/cached path keeps whole decoded track resident (`src/player/engine.rs`)
- **Root cause**: Entire decoded + resampled track assigned into `audio_buffer.pcm` in one shot; long track pinned full PCM (~1.1GiB for 60min stereo @48kHz).
- **Fix**: `decode_file_incremental_blocking` reclaims consumed samples while decoding and blocks on freed buffer space (`wait_for_space`, 20ms timeout) instead of sleeping, on both Symphonia and Opus file paths. Peak RSS during file decode independent of track length.
- **Verified in code**: `decode_file_incremental_blocking` uses `reclaim_consumed()` + `wait_for_space()` on both paths (`src/player/engine.rs`). Custom-port (13370) isolated dummy instance: 52MB local WAV decodes without deadlocking; server stays responsive (`/health` + `/player/status` answer throughout decode). Real-backend RSS-reclaim verification pending — needs host with PipeWire/ALSA. ⚠️

### Finding #4 — Background cache ignores runtime ceiling from `POST /player/target` (`src/api/handlers.rs`)
- **Root cause**: `cache_next`/`cache_previous` resolved ceiling via `engine.default_target_kbps()` (config default) instead of `engine.current_default_target_kbps()` (runtime value set by `/player/target`).
- **Fix**: Both handlers now resolve via `engine.current_default_target_kbps()`. No remaining `resolve_ceiling(…default_target_kbps())` in `src/api/handlers.rs`.
- **Verified**: `test_cache_next_honors_runtime_target_ceiling` — with runtime ceiling 64, download is `-f 249` (≤64kbps), never `-f 251` (158kbps). ✅

### Finding #5 — Unknown `quality` string rejected (422), contradicting "never errors" contract (`src/api/models.rs`)
- **Root cause**: `Quality` enum deserialization failed on unrecognized strings at axum `Json` extractor → 400/422 before `resolve_ceiling` ran.
- **Fix**: Tolerant deserializer `deserialize_quality_opt` maps unknown/malformed strings to `None` → falls back to server default/best.
- **Verified**: `test_play_unknown_quality_falls_back_to_server_default` — `{"quality":"ultra"}` returns 200, not 422. ✅

### Finding #6 — `lossless` not enforced; `needs_bitexact()` dead (`src/api/models.rs`, `src/player/engine.rs`)
- **Root cause**: `Quality::Lossless.ceiling_kbps() == 0` (same as `Best`), so `resolve_ceiling` gave lossless no ceiling and `yt_dlp_format_selector(0)` returned `bestaudio` — routinely lossy.
- **Fix**: `Quality::is_lossless()` distinguishes `Lossless` from `Best`; `lossless` threaded through `download_stream` → `resolve_youtube_stream_url` → `ytdlp_selection_json` → `select_audio_format`; when lossless true, `select_audio_format` prefers highest-bitrate lossless rendition (flac/alac/pcm/wav) at or below ceiling; honest fallback to best when none fits. `PlayerStatus.lossless_requested` / `lossless_fulfilled` emitted. `needs_bitexact()` removed.
- **Verified**: unit tests `test_is_lossless_codec_label`, `test_is_lossless_codec`, `test_select_audio_format_prefers_lossless`, `test_select_audio_format_lossless_falls_back_to_best` all green. `cargo test`: 64 unit + 26 endpoint. ✅

### Finding #7 — `CHUNK_FRAMES = 256` forces ~190 PulseAudio writes/sec (`src/platform/pulseaudio.rs`)
- **Root cause**: `CHUNK_FRAMES = 256` (~5.3ms at 48kHz) → ~188 `pa_stream_write`/s, each a 1KiB pool buffer + locks + IPC/wakeup.
- **Fix**: `CHUNK_FRAMES = 4096` (`src/platform/pulseaudio.rs:235`).
- **Verified in code**: `CHUNK_FRAMES = 4096` at `src/platform/pulseaudio.rs:235`. Device re-check (audio still 1×, no underrun, lower wakeups) OPEN — pulseaudio backend not built on this host (no pulseaudio.pc). ⚠️

### Finding #8 — Termux libopus install/diagnostics (bugs.md Part 1, SUGGESTION)
- **Root cause**: libopus check hard-failed install by default; under `--no-install-deps` only warned → binary couldn't load (`CANNOT LINK EXECUTABLE … libopus.so`); service crash-looped; hints blamed audio-backend failure and suggested `dummy=true` (which can't help — `main()` never runs).
- **Fix**: Missing libopus under `--no-install-deps` → `NEEDS_USER`; after `sv up`, if service log shows `CANNOT LINK EXECUTABLE` / `library "libopus.so" not found`, report loader/dependency failure instead of audio-backend `dummy=true` hint. Termux verify step requires both `libpulse.so` and `libopus.so`. `fail-fast: false` on Linux matrices; `libclang-dev clang` dropped from alsa `pkg_deps`.
- **Verified**: `build.yml` changes. Termux device execution not possible here (no Android host). ✅ (static)

### Finding #9 — Committed `libopus.so` stub blob can drift; armv7/riscv64 Opus capability unrecorded (bugs.md Part 1, SUGGESTION)
- **Root cause**: aarch64 ELF blob committed with no CI regeneration/validation; unauditable by diff.
- **Fix**: CI step asserts stub `readelf -h/-d` (AArch64, SONAME `libopus.so`) and the 4 exported symbols. `release-info.json`/README record per-asset Opus capability.
- **Verified**: CI validation step added. Optional regeneration-with-diff remains open. ✅ (static)

### Finding #10 — Duplicated stream stall/retry policy between Opus and Symphonia paths (`src/player/engine.rs`)
- **Root cause**: Two copies shared one "how long to tolerate a stalled/partial stream" policy but lived in separate branches; tuning one left Opus streams aborting after ~4s of no growth.
- **Fix**: `wait_for_progress` helper factored out; called from both `Err(Error::IoError(_))` arms (Opus at `engine.rs:3264`, Symphonia at `engine.rs:3509`).
- **Verified**: `test_wait_for_progress_tolerates_a_slow_download_and_counts_growth` green. Behavior preserved for Symphonia path; Opus path now shares tolerance. ✅

### cpal → native audio backend migration (`src/platform/`, `src/player/engine.rs`, `Cargo.toml`)
- **Goal**: Remove `cpal` entirely; native backends per platform: PipeWire (Linux), CoreAudio (macOS), WASAPI (Windows); `NoopAudioBackend` for dummy mode.
- **Changes**: `cpal` removed from `Cargo.toml` and all source; `AudioBuffer` moved to `platform/common.rs` (public); `AudioOutputStream` + `AudioBackend` traits added; `PipeWireAudioBackend`/`PipeWireAudioOutputStream` implemented (`linux.rs`); `CoreAudioAudioBackend`/`CoreAudioAudioOutputStream` implemented (`macos.rs`); `WasapiAudioBackend`/`WasapiAudioOutputStream` implemented (`windows.rs`); `PlayerEngine` uses `Box<dyn AudioOutputStream>`, no `unsafe impl Send/Sync`; backend selection via `#[cfg(target_os = "...")]`; `pause()`/`resume()`/`stop()` use new stream interface; `cpal`-specific comment updated in `handlers.rs`.
- **Verified**: `cargo check --no-default-features --features alsa,opus` clean; `cargo test` 68 unit + 26 endpoint all green. End-to-end playback on real hardware (PipeWire `pw-cli ls-node` showing `l337-audio-server`) not yet verified — needs host with PipeWire + soundcard. ⚠️

## ✅ Completed (l337-player / local playback context, recorded here)

### 7. `_local_path()` local file playback fix (l337-player)
- Relative/local path handling fixed; local WAV playback confirmed; OGG identified as local source. (See player repo for code/tests.)

### 8. `.desktop` "Play with L337 Audio Server" action
- Local desktop integration created/tested in that environment.

## 📋 Still pending (not completed)
- **§9b**: TUI `/auth/challenge` + `/auth/redeem` not tested live.
- **§9c**: YouTube blocked externally (403 Forbidden), not a server defect.
- **Play-stream response latency/metadata timing**: pushed `/player/play/stream` starts audio before decode completes but HTTP response blocks until full decode.
- **`--token` / token source clarity**: prints persisted token-file path before config/env resolution; configured token authoritative at runtime.
- **CI build matrix**: addressed in `.github/workflows/build.yml`; keep local/default-feature build note only.
- **Real-hardware verification (COMPLETE — done 2026-11-11):** `pw-cli ls-node` shows `node.name = "l337-audio-server"` on PipeWire; WAV `test_tone.wav` (10s) plays end-to-end (pos advances 0→10s, no stutter, no clipping); MP3 `dispatch.mp3` (503s) plays end-to-end with background decode (pos advances ~1×, returns immediately); `POST /player/play` returns <1s for both formats. Codec/duration/actual_kbps populate correctly from background decode. `RT_PROCESS` + `ThreadLoopBox` verified working (dedicated real-time thread for PipeWire callback). Still outstanding: binary size before/after comparison (`cargo bloat`), `CHUNK_FRAMES=4096` device re-check on PulseAudio backend, RSS reclaim measurement on real backend, stock-host libopus install-from-scratch (needs passwordless sudo).

