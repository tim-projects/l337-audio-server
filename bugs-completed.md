# bugs-completed.md — Completed Bug Fixes and Verified Fixes

Tracks bugs fully resolved and verified. For ongoing/pending issues, see `bugs.md`.
Last updated: 2026-11-11.

## ✅ Completed (l337-audio-server)

### 9. Concurrent playback / `stop()` race (2026-11-11, `src/player/engine.rs`)
- **Root cause**: `PlayerEngine::stop()` was synchronous — it aborted the background streaming download/decode tasks without awaiting them. When `play_track()` called `stop()` then immediately started a new decode, the old decode task could still be running and appending PCM to the shared `AudioBuffer` while the new track's decode also wrote to it. Two producers writing to one buffer caused the same track to be heard twice and the buffer to "jump between the two streams."
- **Fix**: Made `stop()` async. It now sets the cancel flag, aborts and awaits the download handle, then awaits the decode handle with a 5-second timeout, guaranteeing the old producer has fully stopped writing to the shared buffer before a new playback begins — enforcing single-track playback. All internal call sites (`play_track`, `play_pushed`, `load_and_play`, `trigger_next`, `trigger_previous`, `get_status` end-of-track) and the `/player/stop` + `/player/next` handlers updated to `.await`.
- **Verified**: `cargo check --no-default-features --features alsa,opus` passes with no new errors; 64 unit + 26 endpoint tests all green.

### 1. AAC divide-by-zero bug (bugs.md)
- **Root cause**: `decode_to_pcm` in `src/player/engine.rs` could yield `channels=0` for AAC-LC when probed codec params had `channels: None`, causing `pcm.len() / channels` panic.
- **Fix**: Clamp/default channel handling (`channels.max(1)` / safe fallback) at affected decode/duration paths.
- **Verified**: AAC plays (~6s) with no panic; WAV playback unchanged. Live/player APIClient validation; CI ALSA binary confirmed. ✅

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

## ✅ Completed (l337-player / local playback context, recorded here)

### 7. `_local_path()` local file playback fix (l337-player)
- Relative/local path handling fixed; local WAV playback confirmed; OGG identified as local source. (See player repo for code/tests.)

### 8. `.desktop` “Play with L337 Audio Server” action
- Local desktop integration created/tested in that environment.

## 📋 Still pending (not completed)
- **§9b**: TUI `/auth/challenge` + `/auth/redeem` not tested live.
- **§9c**: YouTube blocked externally (403 Forbidden), not a server defect.
- **Play-stream response latency/metadata timing**: pushed `/player/play/stream` starts audio before decode completes but HTTP response blocks until full decode.
- **`--token` / token source clarity**: prints persisted token-file path before config/env resolution; configured token authoritative at runtime.
- **CI build matrix**: addressed in `.github/workflows/build.yml`; keep local/default-feature build note only.
