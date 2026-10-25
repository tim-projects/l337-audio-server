# Android / Termux audio — plan and findings

Status: **bionic is the correct (only) Termux build. No glibc build is needed.**

This doc supersedes the glibc route considered during the debugging session. It
records what was confirmed on this device (Android 16 + Termux, aarch64) and the
current state of the code + installer, so the next debugging step is unambiguous.

## 1. Confirmed: bionic only, glibc not required

Evidence, gathered on the target device (not inferred):

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
   version needs are resolved by Android's libc. It is a bionic Termux package.
4. `pactl` (bionic, interpreter `/system/bin/linker64`) links that same
   libpulse, connects to the running PulseAudio daemon successfully. A bionic
   process demonstrably uses this stack.
5. The server's runtime only ever touches `$PREFIX/lib` via `LD_LIBRARY_PATH`
   (set in the runit run script). The termux-glibc userland at `$PREFIX/glibc`
   is **not** on that path and is irrelevant to the server.

=> glibc is not required for Termux audio. The build matrix keeps the two
existing android variants: `native` (bionic, AAudio only) and `termux`
(bionic + PulseAudio dlopen). All glibc-specific work is removed.

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
unchanged from `HEAD`. `git diff` touches only
`.github/workflows/build.yml` and `scripts/install-android-termux.sh`; every
file under `src/` and `Cargo.toml` is identical to the last commit.

## 3. Installer (current)

`scripts/install-android-termux.sh` (run by `bash install.sh` on Termux, but note
the caveat in section 6):
- downloads asset `l337-audio-server-aarch64-android-termux` (bionic) — no glibc
  asset, no flavor detection.
- validates ELF magic before replacing the binary.
- **stops every running instance before deploying** (mirrors
  `scripts/install-systemd.sh` / `install-linux-alsa.sh` `stop_service_*`):
  `sv down` → `pkill -f "$BINARY_PATH"` → waits up to 10s for exit → `fail`s
  loudly with the surviving PID(s) if anything refuses to stop. A survivor holds
  `runtime/l337.sock`, and the new binary's `remove_stale_socket`
  (`src/main.rs:381`) exits(1) on an actively-listening socket — so the update
  would otherwise look like a no-op and enter a runit crash-loop. This was the
  "old binary still running" symptom.
- then `mv` the new binary, (re)write the runit run script, wait for
  `supervise/ok`, `sv up`, verify `sv status` reports `run:`, and link the socket.

## 4. Remaining defect: silent PulseAudio failure

With `dummy = false` on this device, the current binary (release `2026.10.24-1`
== commit `00696e3`, which **includes** the `pa_sample_spec` ABI fix) does:
- attempt `pulseaudio.rs` dlopen (libs load), then
- `pa_simple_new(...) → NULL`, and
- fall back to AAudio → `AAudio_createStreamBuilder` returns null (Android 16
  console process) → `exit(1)` → runit crash-loop.

The failure is **silent**: `android.rs:219` logs only
`PulseAudio not available …` and discards `start_stream`'s `Err` string, which
is where the real diagnose lives (`Failed to load pa_simple_new: <sym>` vs
`pa_simple_new failed with error <N>`). All five required symbols exist in
`libpulse-simple.so` (`pa_simple_new/write/drain/free/get_latency@@PULSE_0`), so
this is not a symbol-resolution problem; `pa_simple_new` itself is failing —
most likely `PA_ERR_CONNECTIONREFUSED`/`PA_ERR_CONNECTION` (server unreachable
from the bionic process) or an invalid argument, which would also explain the
session's "no connect() syscall" observation for the pre-fix binary.

## 5. Next step (before more code)

Apply the one-line diagnostic in `src/platform/android.rs:219`:
```rust
tracing::warn!("PulseAudio not available on this Android device, falling back to AAudio: {}", err);
```
Then run the server with `dummy = false` and read the logged PA error to pick
the precise cause. Hold off on any audio-backend code change until that line
fires — it disambiguates AAudio-vs-pulseaudio failure modes instantly.

## 6. How to install/test locally (important)

`install.sh` downloads the installer script **from the release tag**
(`raw.githubusercontent.com/<repo>/<TAG>/scripts/install-android-termux.sh`),
so `bash install.sh --pre-release` always runs the script shipped in the latest
release — local edits are only exercised after a release/tag that contains them.
To run the current fixed installer now:

```
bash scripts/install-android-termux.sh --pre-release
```

(That fetches asset `l337-audio-server-aarch64-android-termux`, which exists in
`2026.10.24-1`.) For a from-source debug run without termux-services:

```
sv down l337-audio-server; pkill -f l337-audio-server; sleep 1
env LD_LIBRARY_PATH="$PREFIX/lib" TMPDIR="$TMPDIR" \
  "$HOME/.local/share/l337-player/bin/l337-audio-server" --transport=socket
```
