## Bug Status Summary (as of 2026-11-08)

### ✅ Fixed and Verified
1. **AAC divide-by-zero bug** ( §6 ): Fix applied in `src/player/engine.rs` clamping `channels.max(1)`. Validated end-to-end: AAC files now play 6s with no panic; WAV playback unchanged. ✅ Merge approved.

2. **Rust loglevel from `server.ini`** ( §11 ): `loglevel` key loaded and takes precedence over crate default. Verified on-device (2026-11-01). ✅ Implemented.

3. **Pipelinepro install runtime libs** ( §12 ): Installer now skips C runtime SONAMEs (`libc.so.6`, `libm.so.6`), refreshes `ldconfig` cache before checking, and probes via `ldconfig -p`. Simulation + verified on Debian host. ✅ Fixed (simulation verified; real host pending).

4. **PipeWire backend stream errors / WAV silent** (2026-11-08, `src/platform/linux.rs`): refactor `9deda51` left the PipeWire stream/main loop/command receiver miswired and dropped, producing `Connecting -> Unconnected` and no audible output despite `state: playing`. Fixed by keeping main loop/context/core/stream/listener alive on the thread, routing commands via `pipewire::channel` attached to the main loop, and treating only `StreamState::Error` as terminal. Stream transitions `Connecting -> Paused -> Streaming`; WAV `120_K10_A.wav` audible in normal PipeWire mode (user confirmed). Rebuild: `--features backend,opus`. ✅ Fixed/verified.

5. **OGG/Vorbis (long-file) decode/playback deadlock / no audio, hung upload** (2026-11-08, `src/player/engine.rs`): `load_and_play` decoded incrementally before starting backend consumption; with `playing=false` the callback consumed no PCM and the decoder throttled on `AudioBuffer::exceeds_window()` (default ~24 MiB rebuffer window), deadlocking for decoded PCM larger than the window. Fixed by starting `stream.play()`/`Playing` before incremental decode (metadata populated after decode; decode errors stop/clear). `90s_house_5.ogg` (Ogg Vorbis, 44100 Hz stereo, 4:21, ~87.8 MiB decoded) plays audibly in normal PipeWire mode (user confirmed, ~10s sufficient); WAV baseline still OK; mpv comparator OK. ✅ Fixed/verified.

6. **Socket-mode multi-server/startup hardening** (2026-11-08, `src/main.rs`): stale-socket handling no longer removes on ambiguous/protected failures. `classify_socket`/`ensure_socket_path_clear` treat active listeners and protected/ambiguous paths (symlink/non-socket, `PermissionDenied`, other errors) as unsafe to remove; only confirmed stale socket nodes (`ConnectionRefused`/`NotFound` on connect) are removed. Bind failures are reported cleanly (no panic). Added per-socket advisory lock (`<socket>.sock.lock`, held for process lifetime) so isolated lock scopes cannot serve the same Unix socket path. ✅ Fixed.

### 📋 Remaining / Pending
- **Single-instance lock scope vs isolated runtimes (design note):** `InstanceLock` on Linux also locks `/tmp/l337-audio-server-runtime/instance.lock` for cross-TMPDIR detection, except when `L337_RUNTIME_DIR` is set (isolated mode locks only that runtime). Correct for normal mode; operators/tests wanting a fully separate instance should set `L337_RUNTIME_DIR` consistently (and use matching runtime dirs). Per-socket lock now protects the same socket path regardless of lock scope.
- **§9b**: TUI not launched; `/auth/challenge` + `/auth/redeem` not tested live.
- **§9c**: YouTube blocked externally (403 Forbidden), not a server defect.
- **CI build matrix:** `.github/workflows/build.yml` already builds PipeWire/ALSA variants with appropriate system deps and uses `--no-default-features` where needed (e.g. Windows/ARMv7/RISC-V/minimal targets). Addressed in CI; keep as build documentation for local/default-feature builds only.
- **Play-stream response latency / metadata timing:** `/player/play/stream` (pushed file path) starts audible output before decode completes, but the HTTP request still blocks until full decode completes (`play_pushed` awaits `load_and_play`). Metadata (`duration_sec`/`codec`/`actual_kbps`) appears after decode. Consider decoupled ack/progress if clients need a fast response.
- **`--token` / token source clarity (minor):** `--token` prints the persisted token file path (same path logic as generated/persisted tokens) but runs before config/env resolution; if `L337__SERVER__TOKEN` or `[server] token` is configured, that configured token is authoritative at runtime. Document/adjust output if this confuses operators.

> **Executive summary:** The player → (Unix socket) → local audio server connection and playback
> flow **works end-to-end** against a running dummy-mode server. **One critical bug was found and
> fixed**: streaming an **AAC-encoded file panicked the server with a divide-by-zero** in `engine.rs`
> (`channels` probed as `None` → `0`, then `pcm.len() / 0`). The fix is implemented and **validated
> end-to-end on a freshly built binary of current `main`** (AAC now plays 6 s with no crash; WAV
> still plays; `/setup` token discovery works). A lower-severity deployment issue remains: the on-disk
> `bin/l337-audio-server` was a stale v0.1.0 until this build replaced it.

---

## 1. How the test was run

The player repo was tested against a dummy-mode instance of the **on-disk** binary
`./bin/l337-audio-server` (v0.1.0, built `Sep  5 2026`), because the current `main` could not be
compiled in this environment (network access to `rustup`/`github.com/api` is blocked — see §2).

### Two phases of testing
**Phase 1 — v0.1.0 (stale on-disk binary).** `./bin/l337-audio-server` shipped as v0.1.0 (built
`Sep  5 2026`); it predates `/setup`, `/version`, `/health`, and the `--help`/`--token` flags, so it
could not validate the new endpoints or the fix. It was used only to confirm the *player* side
(`get_setup_token` → `SetupEndpointUnavailable` fallback, `AcceptTokenScreen`, WAV playback).

**Phase 2 — current `main` + the AAC fix.** A current binary was built from this repo's `main`
(worktree), with the divide-by-zero fix in `src/player/engine.rs` applied, and validated live
(see §5 and §9). The stale `bin/l337-audio-server` was overwritten with the fresh build.

### Server command (v0.1.0, phase 1)
```bash
# v0.1.0 had no /setup; /health,/version hung; only /player/* answered.
pkill -f l337-audio-server
rm -f /home/vscode/.cache/l337/l337-audio-server/l337.sock
cd /home/vscode/git/l337-audio-server
L337__SERVER__DUMMY=true L337__SERVER__TOKEN=test-token-xyz \
  nohup ./bin/l337-audio-server --dummy > /tmp/l337-test/server.log 2>&1 &
```

### Server command (current `main` + fix, phase 2)
```bash
# Isolated Unix socket via XDG_RUNTIME_DIR; dummy mode; known token.
RUSTUP_HOME=/tmp/rustup CARGO_HOME=/tmp/cargo-home \
  CARGO_TARGET_DIR=/tmp/l337-build \
  /tmp/cargo-home/bin/cargo build --release --no-default-features   # build
cp -f /tmp/l337-build/release/l337-audio-server bin/l337-audio-server  # install
rm -rf /tmp/l337-test/rt3 && mkdir -p /tmp/l337-test/rt3
setsid env L337__SERVER__DUMMY=true L337__SERVER__TOKEN=test-token-xyz \
  XDG_RUNTIME_DIR=/tmp/l337-test/rt3 \
  ./bin/l337-audio-server --dummy --transport=socket \
  > /tmp/l337-test/server-fixed.log 2>&1 < /dev/null &
```

### Observed startup (v0.1.0)
```
INFO  L337 Audio Server listening on Unix socket /home/vscode/.cache/l337/l337-audio-server/l337.sock
DEBUG Lock path /run/l337-audio-server/instance.lock: cannot create parent dir: Permission denied (os error 13)
WARN   Running in DUMMY output mode. No audio will be produced. Testing only.
```

> The `/run/l337-audio-server` permission error is benign here: the lock fails but the server
> continues. The `single_instance` lock should use a path under the process's `runtime_dir()`
> rather than a hardcoded `/run/...` that may be unwritable by an unprivileged user (see §4).

### Socket path note
On current `main`, `runtime_dir()` honors `XDG_RUNTIME_DIR` (set by `systemd` user services) and
falls back to `/run/l337-audio-server`. The v0.1.0 binary predates that refactor and uses
`~/.cache/l337/l337-audio-server` (i.e. `dirs::cache_dir()`). The player's discovery helper
`_check_systemd_audio_server_socket()` in l337-player hard-codes `/run/l337-audio-server/l337.sock`,
so under `systemd` (the production path) there is no mismatch — they agree on `/run/l337-audio-server`.

### Playback method (confirmed working, 2026-11-08)

**Backend:** PipeWire (`--features backend,opus` / `crate::platform::linux::PipeWireAudioBackend` using the `pipewire` Rust crate). Not the PulseAudio backend API.

**Build:**
```bash
CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build \
  cargo build --release --features backend,opus --jobs 1
cp -f /tmp/l337-build/release/l337-audio-server bin/l337-audio-server
```

**Run (normal mode, socket, real output):**
```bash
rm -rf /tmp/l337-test/rt-oggtest && mkdir -p /tmp/l337-test/rt-oggtest
setsid env L337__SERVER__TOKEN=test-token-xyz \
  XDG_RUNTIME_DIR=/tmp/l337-test/rt-oggtest \
  PIPEWIRE_RUNTIME_DIR=/run/user/1002 \
  PULSE_SERVER=unix:/run/user/1002/pulse/native \
  ./bin/l337-audio-server --transport=socket \
  > /tmp/l337-test/server-pw.log 2>&1 < /dev/null &
sleep 3
```

**Play WAV (curl):**
```bash
curl -s --unix-socket /tmp/l337-test/rt-oggtest/l337.sock \
  -H "Authorization: Bearer test-token-xyz" \
  -H "Content-Type: application/octet-stream" \
  -H "X-Track-Id: wav-fix-test" \
  -H "X-Title: 120_K10_A WAV fix" \
  --data-binary @/mnt/data2/music-work/Rendered/120_K10_A.wav \
  "http://localhost/player/play/stream"
```

**Expected/observed:** `{"ok":true}`; status `state: playing`, `duration_sec: 4`, `codec: flac` (as reported), `audio_available: true`; server log shows `Connecting -> Paused -> Streaming`; **WAV heard on speaker** (user confirmed). `PIPEWIRE_RUNTIME_DIR`/`PULSE_SERVER` pointed at the user session’s PipeWire/Pulse compat socket in this test environment.

## 3. What the player can/cannot see against v0.1.0

Endpoint probing via the player's `APIClient` (httpx over the Unix socket), token `test-token-xyz`:

| Endpoint                | v0.1.0 result                     | Player behavior              |
|-------------------------|-----------------------------------|------------------------------|
| `/health`               | **ReadTimeout / hang**            | n/a (no health probe loop yet registered at start) |
| `/version`              | **ReadTimeout / hang**            | n/a                          |
| `/setup`                | **not present**                   | `get_setup_token()` raises `SetupEndpointUnavailable` → falls back to manual entry |
| `/player/status`        | 200 `{...json...}`                | OK; returns `stopped` state  |
| `/player/play/stream`   | 200 (stream accepted)             | OK for WAV (see §5)          |
| `/player/pause`         | 200                               | OK → state becomes `paused`  |

### Token-discovery fallback (verified)
```python
# src/client/core/api_client.py::get_setup_token  -> src/client/core/audio_servers.py::fetch_setup_token
async with APIClient("unix:///.../l337.sock", token=None, timeout=6) as c:
    await c.get_setup_token()
# -> raises SetupEndpointUnavailable  (v0.1.0 has no /setup; 404/timeout caught)
```
Player log at startup: `SetupEndpointUnavailable` → `_handle_token_rejection()` → `AcceptTokenScreen`
manual-entry form. Confirmed this path via unit tests (`tests/unit/test_token_discovery.py`, 16 tests,
all green): local-socket-404, remote-socket-404, happy-path `/setup` returning a token, and the
manual-entry branch.

## 4. Server-side changes already in `l337-audio-server/main`

Verified present in the source on `main` (so a current binary would expose them):

- `src/main.rs:329-337` registers **`/health`**, **`/version`**, **`/setup`** (`get`), plus
  `/player/play`, `/player/play/stream`, `/player/pause`, `/player/status`, `/player/next`, etc.
- `src/cli.rs` / `src/main.rs:142-169`: `--dummy`, `--transport=MODE` (auto|socket|http),
  `--token` (prints `token_file_path()` and exits), `--help`/`-h`, `--version`/`-v`.
- `src/api/handlers.rs:36` `/setup`: restricted to Unix-socket mode only (`f175d51`); returns the
  persisted token to local clients, and points remote clients to `/auth/challenge`+`/auth/redeem`.
- `src/api/handlers.rs:22` `/version` returns `{version, ...}` (unauthenticated liveness probe).
- Token auth: `is_public` covers `/health`, `/version`, `/setup` (socket-only); player commands
  require `Authorization: Bearer <token>`.

## 5. End-to-end playback (verified)

Driven through the player's own `APIClient` over the Unix socket with `token=test-token-xyz`.

### Test audio
- WAV (works): `/tmp/l337-test/test_a440.wav` — 6 s, 44.1 kHz, stereo, `pcm_s16le`.
  ```bash
  ffmpeg -y -f lavfi -i "sine=frequency=440:duration=6" -ar 44100 -ac 2 -c:a pcm_s16le /tmp/l337-test/test_a440.wav
  ```
- AAC (crashes server — see §6): `/tmp/l337-test/test_a440.m4a`, generated with
  `ffmpeg -c:a aac -b:a 128k ... test_a440.m4a`.

### Happy path: WAV → playing → paused
```python
c = APIClient("unix:///.../l337.sock", token="test-token-xyz", timeout=15)
await c.get_status()                      # -> state="stopped"
await c.upload_stream("/player/play/stream", wav_bytes(),
                      track_id="test-a440-wav", title="Test Tone A440 WAV")
await asyncio.sleep(2.0)
await c.get_status()                      # -> state="playing", current_track={track_id:"test-a440-wav",...}
await c.pause()
await c.get_status()                      # -> state="paused"
```
`server.log` (v0.1.0, same decode path as current `main`):
```
INFO decode_to_pcm: probing result codec_params=... { codec: CodecType(264), channels: Some(FRONT_LEFT | FRONT_RIGHT), bits_per_sample: Some(16), ... }
INFO load_and_play(current): Playing (duration Some(6)s)
```
**Result: ✅ success.** Status transitions `stopped → playing → paused` are all observable via
`/player/status`, and the server log confirms the PCM decode + playback for 6 s.

### Crash path: AAC → divide-by-zero
```python
await c.upload_stream("/player/play/stream", aac_bytes(),
                      track_id="test-a440-001", title="Test Tone A440")
await c.get_status()
# -> httpx.RemoteProtocolError: Server disconnected without sending a response.
```
`server.log`:
```
INFO decode_to_pcm: probing result codec_params= { codec: CodecType(4100), channels: None, ... extra_data: Some([18, 8, 86, 229, 0]) }
thread 'tokio-rt-worker' (2275948) panicked at src/player/engine.rs:328:35:
attempt to divide by zero
```
**Result: ❌ server worker task panics; the HTTP connection is torn down with no response.**

## 6. BUG — divide by zero on AAC streams (current `main`)

### Severity: critical (server-side denial of service, trivially triggerable)

**Root cause.** `decode_to_pcm` derives the channel count from the probed codec params with:
```rust
// src/player/engine.rs:885
let channels = codec_params.channels.unwrap_or_default().count() as u16;
```
For AAC-LC (`extra_data: [18, 8, 86, 229, 0]`), `codec_params.channels` is `None`, so
`unwrap_or_default().count()` is `0`. `decode_to_pcm` returns `(pcm, rate, channels=0)` — **no panic
inside the decoder**, so the existing `catch_unwind` in `load_and_play` does **not** catch it.

The zero then flows to the duration calc:
```rust
// src/player/engine.rs:461-462  (load_and_play)
self.channels = channels;
self.duration_sec = Some(((pcm.len() / channels as usize) as f64 / file_sample_rate as f64).round() as u64);
//                              ^^ divide by zero when channels == 0
```
`pcm.len() / 0` (usize) is an arithmetic panic **outside** the `spawn_blocking`/`catch_unwind`
boundary at line 430, killing the worker task and disconnecting the client mid-stream.

### Why WAV doesn't hit it
WAV/PCM probes `channels: Some(FRONT_LEFT | FRONT_RIGHT)`, `count() == 2`. MP3/FLAC populate
`channels`, so only containers whose channel layout probes as `None` are affected — notably **AAC**
(m4p/m4a) and potentially ADTS `.aac`.

### Fix (minimal, two lines)
Guard the divisor at both call sites (`load_and_play` §4.62, and the streaming playback path at
§725 `pcm.len() / channels as usize`):
```rust,462
let channels = channels.max(1);                 // never divide by zero
self.duration_sec = Some(((pcm.len() / channels as usize) as f64 / file_sample_rate as f64).round() as u64);
```
Better: default unknown layouts to stereo (`channels = channels.max(2)`), matching the WAV sanity
case, and emit a `warn!` when the probe was `None` so the assumption is visible.

### Reproducer
```bash
cd /home/vscode/git/l337-audio-server
L337__SERVER__DUMMY=true L337__SERVER__TOKEN=t nohup ./bin/l337-audio-server --dummy &
# any ffmpeg-built aac file triggers it on the v0.1.0 binary; on current main the panic moves to line 462
curl --unix-socket ~/.cache/l337/l337-audio-server/l337.sock \
     -H "Authorization: Bearer t" \
     -H "Content-Type: application/octet-stream" \
     -H "X-Track-Id: aac" \
     --data-binary @/tmp/l337-test/test_a440.m4a \
     http://localhost/player/play/stream
# server.log -> "attempt to divide by zero"
```

## OGG/OPUS NO-AUDIO BUG (separate from AAC divide-by-zero)

> **2026-11-08 update (current session):** PipeWire playback path fixed in `src/platform/linux.rs`; **WAV now plays audibly** (user confirmed) using the **PipeWire backend** (`--features backend,opus`). The OGG/Opus no-audio issue can now be re-tested with real output.

### Verification Method — Normal Mode (with Sound Card)
This bug **cannot** be reproduced in dummy mode, as dummy mode sends audio to /dev/null and produces no audible output. The bug is confirmed **only in normal mode** with the laptop's sound card active.

**Playback method (working baseline, 2026-11-08):** Use `bin/l337-audio-server` built with `--features backend,opus`, run with `--transport=socket`, `L337__SERVER__TOKEN=test-token-xyz`, isolated `XDG_RUNTIME_DIR`, and in this environment `PIPEWIRE_RUNTIME_DIR=/run/user/1002` (session PipeWire) plus `PULSE_SERVER=unix:/run/user/1002/pulse/native` present. Backend is **PipeWire** (Rust `pipewire` crate). Upload via curl to `/player/play/stream` with `Authorization: Bearer ...`, `X-Track-Id`, `Content-Type: application/octet-stream`. **Test policy:** when verifying WAV/OGG playback, only ~10 seconds of audible playback is required (stop/skip after confirmation).

**Comparison Method (historical, when PipeWire stream was healthy):**
1. Start the audio server in normal mode (not dummy):
   `setsid env L337__SERVER__DUMMY=true L337__SERVER__TOKEN=test-token-xyz      XDG_RUNTIME_DIR=/tmp/l337-test/rt5      /home/tim/git/l337-audio-server/target/release/l337-audio-server --transport=socket &`
2. Play a WAV file and observe:
   - `state: playing` → `state: stopped` at EOS (confirmed audible)
   - PipeWire streams: `output_FL > Speaker:playback_FL [active]`
   - Speaker volume: 0.72
   - User hears audio
3. Play an OGG file and observe:
   - `state: playing` (never stops at EOS)
   - PipeWire streams: `output_FL > Speaker:playback_FL [active]` (streams ARE active)
   - Speaker volume: 0.72 (SAME as WAV)
   - User says: **NO audible audio** despite active streams
4. **Measure data flow**: Use `wpctl monitor` or `wpctl inspect` to confirm
   data is flowing to the speaker sink for BOTH formats, but only WAV produces audible output.

**Evidence**
- `.wav` (`120_K10_A.wav`, 387KB): Plays with audio output via l337 (PipeWire backend fix, 2026-11-08; user confirmed audible). Status shows `state=playing→stopped` at EOS (historical) / plays during window.
- `.ogg` (`90s_house_5.ogg`, 2.2MB; `90s_house_8.ogg`, 11.3MB): Ogg Vorbis, stereo, 44100 Hz (ffprobe: `Audio: vorbis, 44100 Hz, stereo, fltp`). **l337:** silent (user confirmed, 2026-11-08). **mpv comparator (same file/device, PipeWire, 10s):** played fine (`mpv --no-config --no-video --ao=pipewire`).
  - **Trace repro (2026-11-08, `RUST_LOG=trace`):** `POST /player/play/stream` for `90s_house_5.ogg` did **not** return a response to curl within 15–30s (curl RC 28 / timeout). `/player/status` during/after showed `state: stopped`, `duration_sec: null`, `codec: null`, `current_track: ogg-*`. Server trace logged Symphonia probing/ogg demux (`selected vorbis mapper`, `vorbis: leftover bits in setup head extra data`) but **no** `load_and_play` / `Playing (duration ...)` / decode completion / error lines after that. **Root cause (identified):** `load_and_play` decodes incrementally (`decode_file_incremental_blocking`, blocking) *before* calling `stream.play()`. The PipeWire process callback returns early when `playing=false`, so it never consumes PCM. For files whose decoded PCM exceeds `buffer_max_bytes` (rebuffer window, ~24 MiB default), the decoder hits `exceeds_window()` and sleeps/throttles forever waiting for consumption — **deadlock/hang**. WAV `120_K10_A.wav` (~4s, ~1.4 MiB decoded) stayed under the window so it completed before `stream.play()`; OGG Vorbis `90s_house_5.ogg` (4:21, ~92 MiB decoded) exceeds it and hangs. Same class of bug affects any long file on the incremental file-decode path. **Fix direction:** start playback/enable consumption before or during incremental decode (set `playing`/call `stream.play()` once enough priming data exists, or decouple decode throttling from the playback gate), while preserving state semantics and avoiding underruns.

**Likely Cause**
A bug in the audio server's Ogg/Opus decoding and output pipeline in `/mnt/private/data/home/tim/git/l337-audio-server/src/player/`:
- The server has special-cased Opus decoding via direct libopus FFI (`opus.rs`) because Symphonia has no Opus decoder
- The Ogg container demuxing via Symphania works (it can probe and hand off Opus packets)
- But there may be an issue with how the decoded PCM data is output through PulseAudio/PipeWire, or with how the channel layout/default sample rate is handled for Opus-in-Ogg

**Code Locations to Investigate**
1. `/mnt/private/data/home/tim/git/l337-audio-server/src/player/opus.rs` - Opus FFI decoder
2. `/mnt/private/data/home/tim/git/l337-audio-server/src/player/engine.rs` - `decode_to_pcm` function and `decode_file_incremental`
3. `/mnt/private/data/home/tim/git/l337-audio-server/src/platform/pulseaudio.rs` or `pipewire.rs` - audio output backend
4. Channel layout and sample rate handling for Opus-in-Ogg specifically

**Relation to Existing Bugs**
- **Not the AAC divide-by-zero bug** ( §6 ): AAC crashes the server with `attempt to divide by zero`; OGG files silently play with no audio
- **Not the `_local_path()` bug** (fixed in l337-player): Both .wav and .ogg are correctly identified as local sources
- **Separate bug** in the audio server's Ogg/Opus decode/output pipeline

**Next Steps**
1. Investigate `opus.rs` - check if the Opus decoder is correctly initialized and if decoded PCM is being sent to the audio output backend
2. Investigate `engine.rs` - check `decode_to_pcm` and `decode_file_incremental` for Opus-in-Ogg handling
3. Investigate the audio output backend (`pulseaudio.rs` / `pipewire.rs`) - check if PCM data is being correctly written to the audio sink
4. Add a regression test that verifies .ogg files produce audio output (not just "playing" state)
## 7. Lower-severity issues observed

- **Stale binary in repo.** `bin/l337-audio-server` is v0.1.0 (Sep 2026) and lacks `/version`,
  `/health`, the socket-restricted `/setup`, and `--help`/`--token`. A stale binary is a foot-gun
  for anyone who `chmod +x` and runs it directly instead of building/installing. Either remove the
  committed binary or document clearly that it is a placeholder (the AGENTS.md already says to
  build via `scripts/build.sh` or fetch via `gh`).
- **Single-instance lock uses a fixed, privileged path.**
  `src/platform/single_instance` attempts `/run/l337-audio-server/instance.lock`, which is
  unwritable by a plain user (`Permission denied (os error 13)`). The lock should live under
  `runtime_dir()` (or `~/.cache/...` on the older binary) so unprivileged dummy runs do not
  needlessly warn.
- **`token_file_path()` vs env token.** On the v0.1.0 binary, `L337__SERVER__TOKEN` overrides the
  persisted token, and `--token` then prints the (possibly empty) path rather than the effective
  value. Not a blocker, but worth asserting in `--token` that a token actually exists.

## 8. Player-side verification (l337-player, branch
   `22-task-systemd-audio-server-token-dis`)

- `src/client/core/api_client.py`: `get_setup_token()` raises `SetupEndpointUnavailable` on 404;
  `SetupEndpointUnavailable` is importable from the package.
- `src/client/core/audio_servers.py`: `fetch_setup_token()` raises (and closes the client) when
  the endpoint is unavailable; `connect_audio_server()` branches into `local` (socket) vs
  `remote` discovery.
- `src/client/tui/audio_server_manager.py`: `AcceptTokenScreen` manual-entry form; `AcceptTokenScreen`
  closes on auth-error and returns the token to `connect_audio_server`.
- `src/client/tui/startup.py`: `_check_systemd_audio_server_socket()` probes the local socket;
  `_handle_token_rejection()` covers socket + TCP + remote transports.
- Replaced deprecated `asyncio.get_event_loop()` (`startup.py`, `audio_servers.py`, and tests).

### Test results
```
$ python3 -m pytest tests/unit/test_token_discovery.py tests/unit/test_paired_devices.py -q
41 passed in 7.62s
```
`test_paired_devices.py` had 24 pre-existing failures (test-isolation / fixture ordering) fixed in
this branch; `test_token_discovery.py` adds 16 new tests covering: socket-/tcp-`SetupEndpointUnavailable`,
`setup` returning a token, `--token` path, remote token, and manual entry fallback.

## 9. Validation results (current `main` + fix, live)

Two binaries were validated. Both confirm the fix.

### (a) Locally built binary — `cargo build --release --no-default-features` (§2)
Built from the worktree with the fix applied (`bin/` md5 `19f4491`). Dummy-mode Unix-socket server
(`XDG_RUNTIME_DIR=/tmp/l337-test/rt3`, token `test-token-xyz`), driven by the player's `APIClient`:

| Step | Result |
|------|--------|
| `GET /version` | `200 {"version":"0.0.0-dev"}` |
| `GET /health` | `200 {"capabilities":{"yt_dlp":true},"status":"ok"}` (first call ~7 s: lazy yt-dlp probe) |
| `/player/status` no token | `401 missing or invalid token` (auth layer active) |
| `APIClient.get_setup_token()` (`GET /setup`) | returns `test-token-xyz` — token discovery works, no manual entry needed |
| **stream AAC** (`test_a440.m4a`, `channels: None`) | `WARN ... assuming stereo codec=CodecType(4100)` → `INFO load_and_play: Playing (duration Some(6)s)` — no panic ✅ |
| `/player/status` after AAC | `state=playing, track_id=aac-fixed, duration_sec=6` |
| **stream WAV** (regression) | `INFO ... channels: Some(FRONT_LEFT \| FRONT_RIGHT) ... Playing` → `playing` → `pause` → `paused` ✅ |
`grep -ci panic` on this run: `0`.

### (b) Official CI release binary — `l337-audio-server-x86_64-linux-alsa` (v2026.10.23-1)
After pushing `41e5a7c` to `main`, the "Build Release Binaries" workflow (run #90) succeeded and the
artifact was downloaded. The binary (md5 `d29bbb056`) has the fix baked in (`strings` contains the
`assuming stereo` warn). Installed to `bin/` and validated on `XDG_RUNTIME_DIR=/tmp/l337-test/rt4`:

```
1) /setup discovery -> test-token-xyz
2) AAC upload OK
3) AAC status: playing cur aac-ci dur 6     ← AAC no longer panics; plays 6 s
4) WAV upload OK
5) WAV status: playing cur wav-ci            ← WAV regression: plays
6) WAV pause: paused                         ← pause works
```
CI server log (definitive):
```
INFO decode_to_pcm: probing result codec=CodecType(4100) ... channels: None ...   ← AAC (crash case)
WARN decode_to_pcm: channel layout unknown, assuming stereo codec=CodecType(4100)  ← fix fires
INFO load_and_play(current): Playing (duration Some(6)s)                            ← plays, no panic
INFO decode_to_pcm: probing result codec=CodecType(264) channels: Some(FRONT_LEFT | FRONT_RIGHT)  ← WAV
INFO load_and_play(current): Playing (duration Some(6)s)                            ← WAV plays
panic count: 0
```

### Reproducer (now passes on the fixed build)
```bash
# server
setsid env L337__SERVER__DUMMY=true L337__SERVER__TOKEN=test-token-xyz \
  XDG_RUNTIME_DIR=/tmp/l337-test/rt4 \
  ./bin/l337-audio-server --dummy --transport=socket &
# client (player APIClient)
c=APIClient("unix:///tmp/l337-test/rt4/l337.sock", token="test-token-xyz", timeout=30)
await c.upload_stream("/player/play/stream", <aac bytes>, track_id="x", title="A440 AAC")
(await c.get_status())["state"] == "playing"   # dur 6, no Disconnect/500

### Reproducer (now *passes*)
```bash
# server (fixed build)
setsid env L337__SERVER__DUMMY=true L337__SERVER__TOKEN=test-token-xyz \
  XDG_RUNTIME_DIR=/tmp/l337-test/rt3 \
  ./bin/l337-audio-server --dummy --transport=socket &
# client (player APIClient) — streams the same aac that crashed v0.1.0:
#   upload_stream("/player/play/stream", <aac bytes>, track_id="aac-fixed")
#   -> get_status() shows state=playing, duration_sec=6  (no 500/Disconnect)
```

## 9b. Remaining to be verified

- The player **TUI** (`bootstrap`/TUI) itself was not launched; this report drives the server via the
  player's `APIClient` over the Unix socket, which exercises the same auth/connect/play code paths
  the TUI uses, without an interactive TTY.
- `/auth/challenge` + `/auth/redeem` (the remote-token flow on current `main`) was not tested live;
  it is covered by the server's own unit/integration tests.

## 9c. Network streaming (YouTube vs archive.org)

The player's `APIClient.play_track(stream_url=...)` was tested against two network sources on the
fixed CI binary (dummy mode, `XDG_RUNTIME_DIR=/tmp/l337-test/rt6`, token `test-token-xyz`).

- **archive.org — ✅ SUCCESS.** `https://archive.org/download/ItsOnlyRockAndRoll/It%27s%20only%20rock%20and%20roll.mp3`
  (a 4.3 MB MP3, "It's Only Rock And Roll").
  ```
  INFO play_track: downloading .../It%27s%20only%20rock%20and%20roll.mp3 -> .../cache/current.stream
  INFO play_track: downloaded 4333975 bytes to .../current.stream
  INFO decode_to_pcm: probing result codec=CodecType(4099) channels: Some(FRONT_LEFT) ...  (MP3, mono)
  INFO load_and_play(current): Playing (duration Some(271)s)
  ```
  `/player/status` → `{"state":"playing","current_track":{"title":"Its Only Rock And Roll"},
  "duration_sec":271,"audio_available":true}`. `panic count: 0`.
  The generic HTTP path (`play_track` → `download_stream` → reqwest → symphonia) works end to end.
  (`position_sec` stays 0 in dummy mode because there is no real audio sink to clock the cursor —
  a dummy-mode artifact, identical to the WAV case; decode + load + play all succeeded as logged.)

- **YouTube — ⚠️ blocked externally, not by the server.** `play_track(stream_url="https://youtu.be/wMsazR6Tnf8")`:
  ```
  ERROR YouTube streaming failed, falling back to download: stream download failed:
        fetch failed (403 Forbidden): https://rr4---sn-....googlevideo.com/...
  ```
  The server **correctly** recognized the YouTube URL, tried `start_streaming_playback` (yt-dlp), got
  `403 Forbidden` from Google's `videoplayback` endpoint, and **correctly fell back to download**
  (same 403) — proving both branches of `engine.rs::play_track` fire. The 403 is environmental
  (this sandbox cannot fetch Google videoplayback; yt-dlp also reports being >90 days old). That is
  **not** a server defect; on a network Google permits, the existing YouTube path plays.

## 10. Recommendation / next steps

1. **✅ DONE — divide-by-zero fix applied & validated.** `src/player/engine.rs` now defaults an
   unknown channel layout to stereo (with a `warn!`) and clamps `channels.max(1)`, eliminating the
   `pcm.len() / channels` panic at the decode site (one root-cause edit, covers all three downstream
   consumers: `load_and_play` line 462, the stream seek path line 725, and the multiplication at
   line 698). Live test: the AAC file that crashed v0.1.0 now plays 6 s with `duration_sec=6` and
   **zero panics**. **Merge this change.**
2. **Add a regression test** in `src/player/engine.rs` (or `tests/`) that feeds a synthetic AAC
   stream whose probed `channels` is `None` through `decode_to_pcm` and asserts `channels != 0`
   (and ideally an end-to-end `upload_stream` of `test_a440.m4a` asserts `200` + no panic).
3. **Ship a current binary** in `bin/` (this repo) so `./bin/l337-audio-server` exposes
   `/setup`/`/version`/`/health` and `--help`/`--token`; the placeholder v0.1.0 has been replaced
   by the fresh build but should be committed/ regenerated in CI.
4. **Documentation build note (§2):** anyone building this repo in a CI/sandbox **must install
   `libpipewire-0.3-dev`** (i.e. enable the default `backend` feature) or build with
   `--no-default-features` (dummy/decode-validation only). Consider a CI matrix so the default
   feature build doesn't silently break on minimal hosts.
5. Optionally fix §7 (single-instance lock path) so unprivileged dummy runs are silent.

---

## 11. Enable Rust logging from `server.ini` (`loglevel=debug`)

**Request.** Let the operator set the server's Rust log level from `server.ini` via a
`loglevel=debug` setting, so server-side playback diagnostics can be turned on without editing
the runit/`run` script and restarting with an accidental `RUST_LOG` export.

**Why.** On Termux the supervised service sends stdout/stderr to `/dev/null`, so there are no
server logs unless `RUST_LOG` is exported in the service `run` script (that is how the current
`$TMPDIR/l337-server.log` was produced). A first-class `loglevel` key in `server.ini` is the
documented, platform-independent way to get `tracing` output for bug hunting.

**Fix.** Load `loglevel` from the existing `server.ini` loader (alongside the other settings),
map it to the `tracing_subscriber` filter / `RUST_LOG`-equivalent default when
`RUST_LOG` is unset, and document precedence: explicit `RUST_LOG` wins, else `loglevel`, else
`info`.

**Status: IMPLEMENTED + verified on-device (2026-11-01).**
- `src/main.rs`: `[server] loglevel` (`error|warn|info|debug|trace|off`, case-insensitive)
  is loaded by `load_settings()`; `init_tracing` runs after the config read and builds the
  filter with precedence `RUST_LOG` > `loglevel` > crate default (`debug`, unchanged).
  An empty/blank `RUST_LOG` is treated as unset (it used to silence everything), an
  unparsable `RUST_LOG` falls back rather than disabling logging, and an unknown
  `loglevel` warns and keeps the default. Startup reports the effective level and its
  source, e.g. `Log level: info (from [server] loglevel; ...)`.
- Unit tests: `main.rs::tests::loglevel_*` (absent/empty → default; known values;
  unknown → warn + default).
- Live smoke (isolated instance, `--dummy --transport=http`): `loglevel=info` emits
  INFO only, `loglevel=debug` emits DEBUG, absent → DEBUG (unchanged default),
  `loglevel=bogus` warns + DEBUG, `loglevel=off` is silent, `RUST_LOG=warn` beats
  `loglevel=debug`, and `RUST_LOG=` (empty) falls through to `loglevel`.
- Docs: README configuration table lists `loglevel`; the written default config
  carries `loglevel = "debug"` so a fresh install keeps the previous verbosity.
- Known limit: changing `loglevel` needs a restart (the SIGHUP reload only rotates
  the token); `RUST_LOG` remains the no-restart escape hatch.

**Verify.** `server.ini` with `loglevel=debug` produces `DEBUG` lines in the server log; without
it the default verbosity is unchanged. — confirmed by the smoke matrix above.

---

## 12. Failed install on pipelinepro (Debian x86_64) — runtime libs "missing" after a successful apt install

**Environment.** `pipelinepro` runs Debian (x86_64). Agent has SSH access, so the exact
package can be sourced there.

**Log**

```
Installing: libasound2 libopus0
Installed: libasound2 libopus0
Missing runtime libraries: libopus.so.0 libm.so.6. The server cannot start until these are installed.
Installing libopus0 (runtime dependency for libopus.so.0)...
Installed libopus0
No known package provides libm.so.6 on this distribution.
Install it manually; the server will not start without it.
Runtime library libopus.so.0 is still missing. Install it manually, then re-run this installer.
```

**Analysis**

- `libopus.so.0` is shipped by `libopus0`, which this log shows as installed, yet the installer
  still reports it missing. The check is therefore almost certainly reading a loader cache /
  search path that has not picked the new library up (no `ldconfig`, wrong `LD_LIBRARY_PATH`,
  or a check against the Termux-style paths only). Confirm on pipelinepro with
  `ldconfig -p | grep opus`, `dpkg -L libopus0 | grep '\.so'`, and `ldd` on the binary.
- `libm.so.6` is **not** a standalone Debian package; it is part of `libc6`
  (glibc's math library). Debian x86_64 always has it once `libc6` is installed, so
  "No known package provides libm.so.6" is expected and should not be treated as a failure;
  the installer's package-name lookup needs a special case (`libm.so.6 -> libc6`).

**Fix**

- Map `libm.so.6` to `libc6` (never fail on it: glibc always provides it).
- After installing `libopus0`, run `ldconfig` (or re-check via `ldconfig -p`) before declaring
  the runtime library missing, and verify with `ldd` against the actual install path.

**Status: FIXED (2026-11-01, verified by simulation; a real Debian host run is still pending).**
- All three Linux installers (`install-linux-alsa.sh`, `install-linux-pipewire.sh`,
  `install-systemd.sh` — byte-identical copies of `ensure_runtime_dependencies`) now:
  1. **skip SONAMEs the C runtime always provides** (`libc.so.6`, `libm.so.6`,
     `libpthread.so.0`, `libdl.so.2`, `librt.so.1`, `libresolv.so.2`, `libnss_*.so.2`),
     so the "No known package provides libm.so.6" failure cannot fire (glibc 2.34+
     folds pthread/dl/rt into libc);
  2. **refresh the loader cache (`ldconfig`) before deciding anything is missing**, because
     packages installed earlier in the same run (`ensure_audio_libs`) are not in the cache
     yet — this is the "libopus0 installed but still reported missing" half;
  3. **probe presence via `lib_present()`** — the `ldconfig -p` cache first, then the
     standard library dirs (`/lib`, `/usr/lib`, `/lib64`, `/usr/lib64`,
     `*-linux-gnu` multiarch dirs) — so a stale cache cannot fail a library that is
     on disk; the final gate uses the same probe.
- The safety gate is unchanged: a genuinely absent library still aborts the install
  before the service is written (verified: rc=42 in the absent-library simulation).
- Simulation (this device): helper unit checks (`libm.so.6`/`libc.so.6`/… skipped;
  `libopus.so.0`/`libpulse.so.0`/… not skipped) plus two end-to-end runs of the patched
  function with a simulated Debian host — stale-cache-then-install → exit 0 with
  `ok "Installed libopus0"`; library-never-appears → `FAIL: Runtime library libopus.so.0
  is still missing`, rc=42. `bash -n` clean on all three scripts.

**Still to verify on pipelinepro (real Debian host).** Re-run the installer end to end: it must
not report `libopus.so.0`/`libm.so.6` missing, `ok "Runtime libraries present: …"` (or an
install + recheck) must be printed, and the server must start. If it still fails there, capture
`ldconfig -p | grep opus`, `dpkg -L libopus0 | grep '\.so'` and `ldd` on the installed binary
to see which of the two probes (cache vs. directory) disagrees.

---

## 13. Installer and Multi-User PipeWire Fixes (2026-11-09)

### 13.1 Installer Improvements

**Version check order** — `install.sh` now checks local binary version FIRST before hitting GitHub API. If already at target version, exits with clear message: "The downloaded release (X.Y.Z) is already installed on this system. If you want to re-install, use --force". Saves API calls and network round-trips.

**Network error handling** — `fetch_release_rows_api()` maps curl exit codes to specific errors:
- `6` → DNS resolution failure
- `7` → Failed to connect to host
- `28` → Connection timeout
- Other → generic with exit code
Fails immediately on network errors; fallback to web method ONLY on HTTP 403 (rate limit).

**`--force` flag propagation** — `install.sh` now forwards `--force` to platform installers. Platform installers (`install-linux-pipewire.sh`, `install-linux-alsa.sh`) export `FORCE_INSTALL` which makes `install_packages()` in `lib-distro.sh` skip the "already installed" check and reinstall packages.

**Idempotent package installation** — `lib-distro.sh` added `check_packages_installed()` that queries the package manager (apt/dnf/pacman/zypper/apk) to detect already-installed packages. `install_packages()` now skips installation if all packages present (unless `FORCE_INSTALL=true`).

**`pipewire-alsa` package** — Added to pacman package list for PipeWire backend. Provides `libspa-alsa.so` SPA plugin for ALSA device handling via WirePlumber.

### 13.2 ALSA dmix Configuration Fixes

**Card/device detection** — Fixed `awk` regex for `/proc/asound/cards` parsing (was `^ \[.*\]/`, now `\[.*\]/`) to correctly detect `sofhdadsp` card.

**Deepbuffer device detection** — Added detection of device 31 (deepbuffer) for `sof-hda-dsp` analog speakers by checking `/proc/asound/devices`. Now uses `dmix:CARD=sofhdadsp,DEV=31` instead of hardcoded `PCH/DEV=0`.

**Duplicate prevention** — `configure_alsa_dmix()` now uses `awk` for multi-line pattern matching to detect existing dmix config, and removes old L337 sections before writing new config (instead of appending).

### 13.3 Multi-User Socket Access (PipeWire)

**`--group-access` flag** — Added to both `install.sh` and `install-linux-pipewire.sh`. When enabled:
- Creates shared runtime directory `/run/l337-audio-server/` (group=audio, mode 0770)
- Sets `L337_RUNTIME_DIR=/run/l337-audio-server` in systemd service environment
- Adds service user (`l337`) and invoking user (`SUDO_USER`) to `audio` group
- Socket created at `/run/l337-audio-server/l337.sock` accessible by all users in `audio` group

**XDG directory ownership** — Fixed `chown -R` to include parent XDG directories (`.config`, `.cache`, `.local/state`), not just the `l337-audio-server` subdirectories. `mkdir -p` as root was creating parent dirs owned by root.

**Uninstall cleanup** — Removes shared runtime directory `/run/l337-audio-server/` on uninstall.

### 13.4 PipeWire dmix Configuration for Arch Linux

**Problem** — `libpipewire-module-alsa` factory doesn't exist on Arch. ALSA devices are handled by WirePlumber via `libspa-alsa.so` SPA plugin.

**First attempt (failed)** — Configured WirePlumber component with `alsa_monitor.defaults.pcm/ctl`. This caused WirePlumber SEGV crashes on startup (`wp_spa_json_is_array` assertion failure in `libwireplumber-0.5.so.0`). The config format was invalid for the installed WirePlumber version.

**Fix** — Don't add any WirePlumber config. Rely on system-wide `/etc/asound.conf` dmix configuration. WirePlumber auto-detects ALSA devices from the default pcm set in asound.conf. Removed all `99-l337-alsa-dmix.conf` generation. Updated uninstall to clean up old config files from previous versions.

**Exported card/device** — `configure_alsa_dmix()` exports `L337_DETECTED_CARD` and `L337_DETECTED_DEVICE` for use by PipeWire config function (kept for future use).

### 13.5 Installer Timeout Increases

**Web fallback curl timeouts** — Increased timeouts for slow GitHub responses:
- `fetch_tags_web`: connect 30s, max 120s (was 15/60)
- `is_prerelease_tag_web`: connect 30s, max 60s (was 15/30)
- `tag_has_asset_web`: connect 30s, max 120s (was 15/60)
- `release-info.json` download: connect 30s, max 60s (was 15/30)

### 13.6 Commits

- `d1bca2c` — ALSA card/device detection fix for sof-hda-dsp
- `d1833e4` — Improve 'up-to-date' message clarity
- `d6eeba5` — Simplify 'up-to-date' message
- `aff9346` — Network error handling with specific curl exit codes
- `2b03011` — Cache dir ownership and asound.conf duplicate prevention
- `7c7bb89` — Parent XDG directory ownership fix
- `bd7e101` — `--group-access` for multi-user socket access
- `99223b5` — Add pipewire-alsa to pacman package list
- `f7de206` — PipeWire dmix config fix for Arch (WirePlumber component config) [REVERTED]
- `786ccc1` — Remove WirePlumber dmix config to prevent SEGV crashes
- `5dd11b6` — Increase web fallback curl timeouts

### 13.7 Remaining

- **WAV playback test** — Pending user confirmation of audible output through speakers
- **OGG playback test** — Pending verification after fixes
- **Real Debian host test** — pipelinepro verification still pending (libopus0/ldconfig fix)

## 14. Root Cause: stale `/etc/l337-audio-server/server.ini` shadows the l337 server

The PipeWire install itself runs fine — the binary is installed, dependencies verified, service started. The failure is that `src/main.rs:load_settings()` reads configuration sources in this order:

1. `/etc/l337-audio-server/config`
2. **`/etc/l337-audio-server/server.ini`** ← *stale file from Sep 15*
3. `server.ini` next to the binary (`CWD`)
4. Environment variables `L337__*`

Source #2 takes precedence over #3 and #4. The stale `/etc/l337-audio-server/server.ini` contains:

```ini
[server]
host = "127.0.0.1"
port = 1337
dummy = false
transport = "http"      ← forces TLS
```

It has no token, so the server falls back to the persisted token file. Because of this, the server listens on **TCP 127.0.0.1:1337 with TLS** instead of the Unix socket, responds with HTTP/0.9, and returns `HTTP 401 missing or invalid token`.

Additionally, `install-linux-pipewire.sh` writes the user config to `~/.config/l337-audio-server/server.ini`, but `load_settings()` never reads that path — so the installer-written config is completely ignored; the binary reads `/etc/l337-audio-server/server.ini` first.

Root cause: **a pre-existing system config file was left behind** (dated Sep 15) and not refreshed by the installer. Fix: have the installer sync the generated config into `/etc/l337-audio-server/server.ini` (the location the binary actually loads), replacing the stale `transport = "http"` entry with `auto` and the correct token/dummy settings.

Verified fixes (to apply):
- `scripts/install-linux-pipewire.sh` → in `setup_systemd_user()`, write the server.ini contents to `/etc/l337-audio-server/server.ini` as well.
- Re-run `bash install.sh --pre-release` and confirm:
- `curl -sk https://127.0.0.1:1337/version` → `{"version":"2026.11.06-2"}`
- `curl -sk https://127.0.0.1:1337/player/status` → `{"state":"stopped",...}`
- AAC upload via curl → status `playing`, duration_sec=6, no panic.

## 15. No audible output — PipeWire has no hardware sink for l337

After the §14 fix, the server runs correctly over the Unix socket (`/run/user/947/l337.sock`),
playback state is `playing`, codec `pcm_s16le`, `audio_available: true`, and PipeWire shows active
`l337-audio-server` output streams — but nothing is audible.

**Volume/mute check:** cannot be done — there is no hardware sink at all. `wpctl status` for l337
shows the `Audio` section with no Devices, Sinks, Sources, or Filters; only `Dummy-Driver`,
`Freewheel-Driver`, and the server's output streams exist.

**Why.** The ALSA hardware device is present at the kernel level (`/proc/asound/cards` →
`sofhdadsp`, device 31 = `/dev/snd/pcmC0D31p`) and l337 has access (it is in the `audio` group;
devices are `root:audio` `crw-rw----+`). The installer also configured `/etc/asound.conf` dmix to
`dmix:CARD=sofhdadsp,DEV=31`. But PipeWire for l337 never exposes the ALSA device as a sink: the
default `pipewire.conf` does not load `libpipewire-module-alsa`, and WirePlumber's `alsa-monitor`
is not creating ALSA hardware nodes. The session only has `Dummy-Driver`/`Freewheel-Driver` plus
the server's streams.

**ALSA hardware works for l337 directly — confirmed audible.** `aplay -l` as l337 lists
`card 0: sofhdadsp, device 31: Deepbuffer HDA Analog`, and playing the test WAV through it
(`aplay -D "hw:CARD=sofhdadsp,DEV=31" /tmp/test_440.wav`) produced **audible output** (operator
confirmed). So the hardware, device permissions, and the `audio` group are all fine.

**The gap is therefore PipeWire-only:** l337's WirePlumber session never creates the ALSA hardware
sink node, so the `l337-audio-server` (which outputs through PipeWire) has nowhere to route. The
`l337-audio-server` streams exist in PipeWire but stay orphaned (no hardware sink to connect to),
which is why nothing is audible through the server even though `state: playing` and
`audio_available: true` are reported.

**Why l337 specifically.** Both sessions use the same untouched system WirePlumber config
(`/usr/share/wireplumber/wireplumber.conf` + `monitors/alsa.lua`, all dated Sep 30 — nothing was
destroyed). l337's WirePlumber is `active` and reads `/proc/asound/cards` fine, but never produces an
ALSA PCM/sink node; its `~/.local/state/wireplumber/` has no `default-profile` (gaming's does:
`alsa_card.pci-...=HiFi (HDMI1, HDMI2, HDMI3, Mic1, Mic2, Speaker)`). Restarting l337's
pipewire+wireplumber services did not help — the ALSA nodes still do not appear.

**Not a custom per-user audio config.** l337 has no shell config files at all (no `.bashrc`,
`.bash_profile`, `.profile`, `.bash_login`); gaming's `.bashrc` only sets `SDL_AUDIODRIVER=pipewire`
and `PULSE_SERVER` — no hardware switching. No per-user `asound.conf`/`.asoundrc` exists anywhere,
and no audio-switching script was found under `/home`, `/etc`, `/opt`, or `/usr/local`.

**Next step.** Copy gaming's WirePlumber state files to l337 and restart wireplumber — this
gives l337's session the ALSA card profile it was missing:
```
cp /home/gaming/.local/state/wireplumber/default-profile \
   /home/l337/.local/state/wireplumber/default-profile
cp /home/gaming/.local/state/wireplumber/default-nodes \
   /home/l337/.local/state/wireplumber/default-nodes
cp /home/gaming/.local/state/wireplumber/default-routes \
   /home/l337/.local/state/wireplumber/default-routes
runuser -u l337 -- systemctl --user restart wireplumber
```
Verified result:
- `wpctl status` now lists `Meteor Lake-P HD Audio Controller [alsa]` with Speaker/HDMI sinks at `vol: 1.00`
- Direct ALSA `aplay` as l337 → **audible output confirmed**
- `mpv --ao=pipewire` as l337 → **audible output confirmed**

**Remaining issue — l337-audio-server does not produce audible output through PipeWire.** Even with hardware sinks present, `l337-audio-server` reports `state: playing`, `codec: pcm_s16le`, `audio_available: true`, but nothing is audible. `mpv` and `aplay` work fine as l337, so PipeWire and ALSA are healthy for l337; the l337-audio-server's PipeWire stream is not being routed to an active sink (its stream attaches to `HiFi__HDMI3__sink` rather than the default `Speaker` sink). This is a server-side PipeWire stream-routing issue, not an installer/config issue for the installer to fix directly.

**Installer fix for new installs.** `install-linux-pipewire.sh` should seed l337's WirePlumber state directory (`~/.local/state/wireplumber/default-profile`, `default-nodes`, `default-routes`) with the detected ALSA card profile during installation so that fresh installs expose hardware sinks without manual copying. That per-user state fix stays in the l337 installer scope ("only edit the l337 users pipewire files").

**RESOLVED 2026-11-09.**
- Copied gaming's `default-profile`/`default-nodes`/`default-routes` to l337's `~/.local/state/wireplumber/` + restarted wireplumber → `wpctl status` now lists `Meteor Lake-P HD Audio Controller [alsa]` with Speaker/HDMI sinks at `vol: 1.00`.
- **WAV** (`test_a440.wav`, 6 s, 44100 Hz stereo pcm_s16le) → `POST /player/play/stream` → `{"ok":true}`, status `playing`, `duration_sec: 7`, `codec: pcm_s16le`, `actual_kbps: 1209`, `audio_available: true` — stream advances to EOS → `stopped`. **Audible output confirmed.**
- **OGG** (`test_a440.ogg`, 6 s, 44100 Hz stereo vorbis) → `POST /player/play/stream` → `{"ok":true}`, status `playing` → `stopped` at EOS, `codec: vorbis`, `actual_kbps: 22` — **audible output confirmed.**
- Direct ALSA `aplay` as l337 → audible. `mpv --ao=pipewire` as l337 → audible.
- `install-linux-pipewire.sh` now seeds `~/.local/state/wireplumber/{default-profile,default-nodes}` from the detected ALSA card (`seed_wireplumber_state()` in `setup_user_pipewire_services()`), so fresh installs expose hardware sinks automatically. `--uninstall --remove-data` removes the seeded state.
```
