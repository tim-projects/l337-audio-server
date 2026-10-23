# bug.md — Audio Server End-to-End Verification

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

## 2. Building current `main` locally (resource findings)

This environment is tight: **`nproc` = 1**, **5.8 GiB RAM / ~970 MiB free** at compile time, and
**no root sudo** (so no extra swap). Two real blockers were discovered and worked around:

1. **Stale lock markers kill resume.** When a release build is OOM-killed mid-link, `build.sh`'s
   `try_cargo` buffers cargo's output in a shell variable, so the failure is silent — the log just
   *stops* at "Running: cargo build --release" with exit 0 reported by the wrapper, yet no binary
   appears and `.fingerprint` freezes. **Fix:** delete the stale
   `release/.cargo-lock`, `release/.cargo-build-lock`, `release/.cargo-artifact-lock` before
   retrying, and run `cargo build` **directly** (not via `build.sh`) so output streams live and
   progress is measurable via `ls release/deps | wc -l`.
2. **`libspa-sys` requires `libpipewire-0.3-dev`.** The default feature set
   (`default=["backend"] → backend=["pipewire","coreaudio-rs","wasapi"]`) pulls `pipewire`→
   `libspa-sys`, whose `build.rs` runs
   `pkg-config --libs --cflags libpipewire-0.3` and **panics** if the system library is absent
   (`Cannot find libraries: PkgConfig(...)`). There is no `libpipewire-0.3.pc` here and no root to
   `apt install libpipewire-0.3-dev`. **Fix for a dummy-only build:**
   `cargo build --release --no-default-features` — this drops the `pipewire` backend entirely.
   Dummy mode (`--dummy`) does not touch the audio-output backend, so the AAC **decode** path
   (the subject of this bug) is still exercised and the resulting warnings about the missing
   backend are harmless.

### Working single-line build (this repo, dummy validation)
```bash
# Toolchain was bootstrapped once via scripts/build.sh into /tmp (rustup root /tmp/rustup, CARGO_HOME /tmp/cargo-home).
RUSTUP_HOME=/tmp/rustup CARGO_HOME=/tmp/cargo-home \
  CARGO_TARGET_DIR=/tmp/l337-build \
  /tmp/cargo-home/bin/cargo build --release --no-default-features --jobs 1
cp -f /tmp/l337-build/release/l337-audio-server bin/l337-audio-server
```
> ⚠️ Do **not** run this with default features in this environment — it will fail at `libspa-sys`
> (missing `libpipewire-0.3-dev`). Install that system package (or build on a host that has it)
> for a binary with a real audio backend. `--no-default-features` is suitable for validating
> the decode path / dummy mode only.

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
