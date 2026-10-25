# Android / Termux audio — plan and findings

Status: **bionic is the correct (only) Termux build. No glibc build is needed.**
PulseAudio root cause narrowed to **dlopen vs. linkage**; final confirmation
requires the rebuilt (diagnostic) binary. **A new release + reinstall is required
before further on-device testing of the PulseAudio fix.**

## 1. Confirmed: bionic only, glibc not required

Evidence, gathered on this device (not inferred):

1. `/system/bin/linker64` is the dynamic linker for **both** the server binary
   (`$PREFIX/../.local/share/l337-player/bin/l337-audio-server`) and `pactl`.
   Both are bionic ELFs.
2. `/system/lib64/libc.so` (Android bionic libc) **defines** the
   `.gnu.version_d` symbol versions `LIBC`, `LIBC_N` … `LIBC_V`. Therefore the
   `.gnu.version_r` "LIBC v3/5/6" needs found in `libpulse.so` are standard
   NDK/bionic packaging, **not** a glibc marker. (The earlier conclusion that
   "version needs ⇒ glibc" was a misread.)
3. `$PREFIX/lib/libpulse.so` NEEDED list contains only bionic sonames
   (`libc.so`, `libdl.so`, `libdbus-1.so`, `libpulsecommon-17.0.so`) whose
   version needs are resolved by Android's libc. It is a bionic Termux package
   (`pulseaudio: 17.0-5`). There is exactly **one** libpulse on the device — no
   glibc duplicate (`find` over `$PREFIX/lib` and `$PREFIX/glibc/lib` finds only
   the `pulseaudio` package's files).
4. `pactl` (bionic, interpreter `/system/bin/linker64`) links that same
   libpulse, connects to the running PulseAudio daemon successfully. A bionic
   process demonstrably uses this stack. Daemon `/proc/<pid>/maps` shows it loads
   `/apex/com.android.runtime/lib64/bionic/libc.so`.
5. The server's runtime only ever touches `$PREFIX/lib` via `LD_LIBRARY_PATH`
   (set in the runit run script). The termux-glibc userland at `$PREFIX/glibc`
   is **not** on that path and is irrelevant to the server. (The
   `ldd …/usr/glibc/lib/libc.so: invalid ELF header` line seen in this session
   is a `ldd` wrapper search-order artifact — Termux's `ldd` is itself a
   glibc-prefix helper binary: it matched `libpulse.so`'s NEEDED `libc.so` name
   against the glibc prefix's file first. It is **not** evidence that libpulse
   is glibc, and it is not the server's code path.)

⇒ glibc is not required for Termux audio. The build matrix keeps the two
existing android variants: `native` (bionic, AAudio only) and `termux`
(bionic + PulseAudio). All glibc-specific work is removed.

## 2. Build configuration (current, correct)

- `Cargo.toml`: `pulseaudio = ["backend"]`. `--features pulseaudio` therefore
  also enables `backend`, which is what selects the Android backend at the engine
  layer.
- `build.yml` (`build-android`):
  - matrix `variant: [native, termux]`, target `aarch64-linux-android`, NDK r28b,
    linker `aarch64-linux-android29-clang`.
  - `termux` variant: `cargo build --release --target aarch64-linux-android --features pulseaudio`
    → produces `l337-audio-server-aarch64-android-termux`.
  - `native` variant: no features → `l337-audio-server-aarch64-android`.
  - Only the `android` job has `continue-on-error` removed (other platforms
    untouched); all other workflows unchanged.
- `src/platform/{mod.rs,android.rs,engine.rs,pulseaudio.rs}`:
  - `android.rs` `start_stream` tries PulseAudio via `pulseaudio.rs` first, falls
    back to AAudio.
  - `pulseaudio.rs` loads `libpulse-simple.so[.0]` with `libloading` and resolves
    `pa_simple_new/write/drain/free/get_latency`.

No other platform builds are affected: the linux/macos/windows/alsa gates are
unchanged from `HEAD`.

## 3. Installer (fixed, verified end-to-end)

`scripts/install-android-termux.sh` (committed in `82a553c`):
- downloads asset `l337-audio-server-aarch64-android-termux` (bionic) — no glibc
  asset, no flavor detection.
- validates ELF magic before replacing the binary.
- **stops every running instance before deploying** (mirrors
  `scripts/install-systemd.sh` / `install-linux-alsa.sh` `stop_service_*`):
  `sv down` → `pkill -f "$BINARY_PATH"` → waits up to 10s for exit → `fail`s
  loudly with the surviving PID(s) if anything refuses to stop. A survivor holds
  `runtime/l337.sock`, and the new binary's `remove_stale_socket`
  (`src/main.rs:381`) exits(1) on an actively-listening socket — so the update
  would otherwise look like a no-op and enter a runit crash-loop.
- then `mv` the new binary, (re)write the runit run script, wait for
  `supervise/ok`, `sv up`, verify `sv status` reports `run:`, and link the socket.
- The run script keeps `LD_LIBRARY_PATH="$PREFIX/lib"` — **required**, see §4.5.
- Requirement checks include `pulseaudio` installed + daemon running.
- **Manually tested on device** (`bash scripts/install-android-termux.sh --pre-release`):
  clean install, exactly one running process after `sv up`, no
  "Socket still active after sv down" warning, socket served.

## 4. PulseAudio failure: evidence chain and root cause

The one-line diagnostic was applied to `src/platform/android.rs:219`:
`tracing::warn!("PulseAudio not available on this Android device, falling back to AAudio: {}", err);`
This is **not in the installed release binary** (`2026.10.24-1` == commit
`00696e3`), which is why the error strings below come from C probes instead.
All probes are bionic, on-device, same uid, against a debug daemon
(`~/padebug.log`).

### 4.1 dlopen path fails, spec-independently

`~/pa_probe2.c`: `format ∈ {S16LE(2), S32LE(3), FLOAT32LE(5)}` ×
`rate ∈ {8000, 22050, 44100, 48000, 96000}` × 2ch → **all 15 combos NULL**,
`pa_strerror(err)` = **"Invalid argument"** (`PA_ERR_INVALID`, code 3).
Includes the daemon's own default spec (44100/S16LE/2ch).
So the failure is **not** the sample spec, **not** its layout
(`{int32 format; uint32 rate; uint8 channels}` matches
`$PREFIX/include/pulse/sample.h`), and **not** symbol resolution (all five
`@@PULSE_0` exports present in `libpulse-simple.so`).

Persistence across environments: default env / explicit
`PULSE_SERVER=unix:<socket>` / `+PULSE_COOKIE=$HOME/.config/pulse/cookie` /
`PULSE_LOG=debug PULSE_LOG_TARGET=stderr` — all identical INVALID, no extra
client-side log output.

### 4.2 The failing layer is `pa_stream_new`

`~/pa_stream2.c`: with a fully READY context (`ctx_state=4`), **every**
`pa_stream_new` / `pa_stream_new_with_proplist` argument combination returns NULL
(errno 0). `pa_simple_new` internally creates a stream, so this is where its
`PA_ERR_INVALID` surfaces. The daemon debug log shows the failing clients die
**client-side with no native-protocol connection at all** — `pactl` connections
log fine (protocol 35, SHM negotiated); the probe gets no entry.

### 4.3 `pa_context_*` and the daemon are fine

`~/pa_ctx_probe.c` (fixed — `pa_context_connect`'s 4th argument is
`pa_mainloop_get_api()`, not `0`; the previous NULL caused the segfault):
`pa_mainloop_new` OK → `pa_context_new` OK → `pa_context_connect` → 0.
`pactl info` always worked. This eliminated the earlier theories:
no thread-spawn failure, no context allocation failure, no server-string
resolution failure.

### 4.4 Audio output itself works (decoupled proof)

`paplay ~/test440.wav` (44100 Hz s16le stereo, generated on-device) → exit 0;
daemon log: "Drain acknowledged of '/data/data/…/test440.wav'", sink
RUNNING → IDLE, sink-input created and freed normally.

⇒ The daemon, the sink (`OpenSL_ES_sink`) and the playback path are all
healthy. The defect is purely in the **client-init path when the library is
opened with dlopen**.

### 4.5 ROOT CAUSE: dlopen vs. linkage (DT_NEEDED)

| call path | result |
|---|---|
| `dlopen("libpulse-simple.so")` + dlsym `pa_simple_new` | NULL, PA_ERR_INVALID |
| `dlopen("libpulse-simple.so")` + dlsym `pa_stream_new` | NULL |
| binary **normally linked** (`-lpulse-simple -lpulse`), direct calls (`~/pa_linked.c`) | `pa_simple_new` **OK**, `pa_stream_new` **OK** |
| DT_NEEDED-linked binary, calls routed **through dlsym** (`~/pa_exact.c`: `dlopen .so.0` fails → `dlopen .so` OK → dlsym → call) | `pa_simple_new` **OK** |

So the PA client library must be loaded as a **linkage dependency at process
start**, not opened later with `dlopen`. Two supporting bionic quirks observed:

1. There is **no `libpulse-simple.so.0` symlink** on the device (only
   `libpulse.so.0` is symlinked), so the server's first dlopen attempt
   (`.so.0`) always fails and the `.so` fallback is the one that matters.
2. Creating the client objects appears to depend on the library's startup-time
   initialization — present when the loader maps it as DT_NEEDED, absent for a
   later dlopen.

### 4.6 What the patchelf probe on the server binary showed

Backup kept at `$BIN.pre-patch` (device, outside repo). Running
`patchelf --add-needed libpulse-simple.so --add-needed libpulse.so` on the
installed binary:

- Startup now loads both libs — without `LD_LIBRARY_PATH=$PREFIX/lib` the
  loader refuses with CANNOT LINK; with it, the server starts (the run script's
  export is therefore mandatory, §3). `LD_DEBUG=all` confirms libpulse activity
  at startup.
- **But the server still fell back to AAudio.** This cannot be attributed
  precisely because the installed build predates the `{} err` diagnostic:
  `pulseaudio.rs::start_stream` returns `Err(String)` which the old binary
  discards. This is the single measurement the next build must provide.

### 4.7 Consequences on device (current state at time of writing)

- `dummy = false` → PA backend skipped, AAudio `AAudio_createStreamBuilder`
  returns null → server `exit(1)` (runit crash-loop risk).
- Service currently **down**; a foreground debug PA daemon is running.
- The installed binary is **temporarily patchelf-modified** (backup exists).

## 5. Fix plan

1. **Rebuild via GitHub Actions** from a commit containing the applied
   `android.rs:219` diagnostic (see §6). This release is the measurement step.
2. Install it, run with `dummy = false`, read the logged Err string. It
   disambiguates instantly:
   - `"Failed to load libpulse-simple…"` → loader/search problem (the missing
     `.so.0` symlink). Fix by preferring `libpulse-simple.so` first in
     `pulseaudio.rs`, or ship the symlink.
   - `"pa_simple_new failed with error N"` → client-init problem (§4.5).
3. For the client-init case, the fix is small and belongs in CI. Add a build
   step after the termux cargo build in `build.yml`:
   ```
   patchelf --add-needed libpulse-simple.so --add-needed libpulse.so
   ```
   (`ubuntu-latest` runners provide patchelf). The Rust dlopen code can stay
   as-is once the library is preloaded at startup, but `pulseaudio.rs` should be
   simplified to try `libpulse-simple.so` **first** — `libpulse-simple.so.0`
   does not exist on Termux, so that attempt is dead weight.
4. Independently of the above, ship the missing symlink (installer or run
   script): `ln -sf libpulse-simple.so "$PREFIX/lib/libpulse-simple.so.0"`.
   It removes one known-failing dlopen attempt and makes the platform look like
   every other Linux.
5. Keep `LD_LIBRARY_PATH="$PREFIX/lib"` in the run script (§3) — removing it
   breaks startup outright (CANNOT LINK) once libpulse is a DT_NEEDED.
6. **Robustness fix regardless of PA outcome:** when PA is unavailable *and*
   AAudio returns null, the server currently `exit(1)`s from the engine
   constructor. Either keep `dummy` required (explicit option per project
   policy) or propagate a clear "no audio backend available" error — but do not
   leave the AAudio-null path silent.

## 6. Build/install/test loop (release required)

- Any source change → commit → push (auto-release) **or**
  `gh workflow run "Build Release Binaries"` → `gh run download <run-id>`.
- `install.sh --pre-release` fetches the installer **script and asset from the
  release tag**, so locally fixed scripts/binaries only take effect after a
  release that contains them.
- The device's installed binary is `2026.10.24-1` (commit `00696e3`).
  **A new release download + reinstall is required before further testing.**

Foreground debug run (needs `LD_LIBRARY_PATH`):
```
sv down l337-audio-server; pkill -f l337-audio-server; sleep 1
env LD_LIBRARY_PATH="$PREFIX/lib" TMPDIR="$TMPDIR" \
  "$HOME/.local/share/l337-player/bin/l337-audio-server" --transport=socket
```

## 7. Device state to restore (housekeeping)

- Installed binary is currently **patchelf-modified** (backup:
  `~/.local/share/l337-player/bin/l337-audio-server.pre-patch`) — restore it or
  replace it via the new release.
- `patchelf` was `pkg install`ed on-device for this test — uninstall if
  undesired.
- Service is **down**; foreground debug PA daemon running (log `~/padebug.log`).
  When finished: `pkill pulseaudio`, restore `dummy = true` in
  `$PREFIX/var/service/l337-audio-server/server.ini`, `sv up l337-audio-server`,
  so the phone is left serving sockets normally.

Probe artifacts (all in `$HOME` or `$TMPDIR`, outside the repo — do not commit):
`pa_probe.c`, `pa_probe2.c`, `pa_probe3.c`, `pa_ctx_probe.c`,
`pa_stream_probe.c`, `pa_stream2.c`, `pa_deep.c`, `pa_linked.c`, `pa_exact.c`,
`pa_dtneeded_test.c`, `dlopen_probe.c`, `test440.wav`, `padebug.log`.
