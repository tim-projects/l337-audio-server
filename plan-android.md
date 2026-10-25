# Android / Termux audio — plan and findings

Status: **bionic is the correct (only) Termux build. No glibc build is needed.**
PulseAudio root cause: **dlopen-only always fails**; a build-time-linked binary
works. The remaining question is the *minimal* ELF condition, plus a pragmatic
fallback that needs no native linking at all (see §5.3).

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
  - termux builds now run a post-link step `patchelf --add-needed
    libpulse-simple.so --add-needed libpulse.so` (verified in the release
    artifact, see §4.8) so the loader maps libpulse at startup.
- `src/platform/{mod.rs,android.rs,engine.rs,pulseaudio.rs}`:
  - `android.rs` `start_stream` tries PulseAudio via `pulseaudio.rs` first, falls
    back to AAudio, and now **logs the real `Err` string**.
  - `pulseaudio.rs` dlopens `libpulse-simple.so` **first** (fixed soname order) and
    resolves `pa_simple_new/write/drain/free/get_latency`.

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
- The run script keeps `LD_LIBRARY_PATH="$PREFIX/lib"` — **required**, see §4.8.
- Requirement checks include `pulseaudio` installed + daemon running.
- **Manually tested on device twice** (`bash scripts/install-android-termux.sh --pre-release`):
  clean install, exactly one running process after `sv up`, no
  "Socket still active after sv down" warning, socket served. Second run (for
  `2026.10.25-1`) verified the installed md5 equals the release asset.

## 4. PulseAudio failure: evidence chain

### 4.1 dlopen path fails, spec-independently

`~/pa_probe2.c`: `format ∈ {S16LE(2), S32LE(3), FLOAT32LE(5)}` ×
`rate ∈ {8000, 22050, 44100, 48000, 96000}` × 2ch → **all 15 combos NULL**,
`pa_strerror(err)` = **"Invalid argument"** (`PA_ERR_INVALID`, code 3).
Includes the daemon's own default spec (44100/S16LE/2ch).
Persistence across environments: default env / explicit
`PULSE_SERVER=unix:<socket>` / `+PULSE_COOKIE` / `PULSE_LOG=debug` — all
identical INVALID, no extra client-side log output.

### 4.2 The failing layer is `pa_stream_new`

`~/pa_stream2.c`: with a fully READY context (`ctx_state=4`), **every**
`pa_stream_new` / `pa_stream_new_with_proplist` argument combination returns NULL
(errno 0). `pa_simple_new` internally creates a stream, so this is where its
`PA_ERR_INVALID` surfaces.

### 4.3 `pa_context_*` and the daemon are fine

`~/pa_ctx_probe.c` (fixed — `pa_context_connect`'s 4th argument is
`pa_mainloop_get_api()`, not `0`; the previous NULL caused the segfault):
`pa_mainloop_new` OK → `pa_context_new` OK → `pa_context_connect` → 0.

### 4.4 The failing client never reaches the daemon

With the debug daemon running (`~/padebug.log`, `--log-level=debug`), a failing
probe adds **zero** lines to the log (817 → 817). `pactl`/`paplay` connections
log fully. ⇒ the failure is **client-side, before any native-protocol
connection** is opened.

### 4.5 Audio output itself works (decoupled proof)

`paplay ~/test440.wav` (44100 Hz s16le stereo, generated on-device) → exit 0;
daemon log: "Drain acknowledged", sink RUNNING → IDLE, sink-input created and
freed normally.

⇒ The daemon, the sink (`OpenSL_ES_sink`) and the playback path are healthy.
The defect is purely in the **client-init path when libpulse is opened with
dlopen**.

### 4.6 ROOT CAUSE (refined): dlopen vs. build-time linkage

| call path | result |
|---|---|
| `dlopen("libpulse-simple.so")` + dlsym `pa_simple_new` | NULL, PA_ERR_INVALID |
| `dlopen("libpulse-simple.so")` + dlsym `pa_stream_new` | NULL |
| binary **built-time-linked** (`-lpulse-simple -lpulse`), direct calls (`~/pa_linked.c`) | `pa_simple_new` **OK**, `pa_stream_new` **OK** |
| built-time-linked binary, calls routed **through dlsym** (`~/pa_exact.c`, `~/pa_m2.c`) | `pa_simple_new` **OK** |
| `dlopen` inside the **released Rust binary** (which now has DT_NEEDED + BIND_NOW + PIE) | NULL, PA_ERR_INVALID |

So a binary **linked against the real Termux libpulse at build time** succeeds
with the identical dlopen/dlsym sequence; a dlopen-only process always fails.
`dladdr` confirms dlsym returns the correct `pa_simple_new` from
`libpulse-simple.so` in both classes — the *address* is right, the library's
internal state is not.

### 4.7 What the 2026.10.25-1 release measured

Installed on device (md5 `4051d2f3…`, matches the release asset), run with
`dummy = false`:
```
WARN android: PulseAudio not available … falling back to AAudio: pa_simple_new failed with error 3
ERROR: Failed to initialize audio device: AAudio_createStreamBuilder returned null
```
Two facts:
1. **`dlopen` now succeeds** — the soname-order fix works; the error is not a
   library-loading failure.
2. **DT_NEEDED patchelf is NOT sufficient** — the release binary has
   `NEEDED [libpulse-simple.so, libpulse.so]` (verified with `readelf -d` on the
   downloaded asset), `BIND_NOW`, PIE, and `LD_LIBRARY_PATH` exported by the run
   script, yet `pa_simple_new` still fails client-side.

### 4.8 Exhaustive bisect — what the difference is NOT

All probes bionic, on-device, same uid, same running daemon:

| experiment | setup | result | rules out |
|---|---|---|---|
| `~/pa_patcheltest.c` | unlinked binary + `patchelf --add-needed` both libs + dlopen | **fails** | patchelf NEEDED alone (reproduces the Rust failure exactly) |
| `~/pa_stubtest.c` | **linked against a stub** `libpulse-simple.so` (SONAME set; stub never present at runtime) | **fails** | link-time symbol *references* / UND entries |
| `~/pa_b4` | linked against a **versioned** stub (`--version-script PULSE_0`), UND `pa_simple_new@PULSE_0` present, BIND_NOW | **fails** | versioned UND refs + verneed |
| `~/pa_linkorder.c` | real-linked, `dlopen .so.0` first vs `.so` first, with/without `TRY_MISSING` | **fails** (both orders) | dlopen order / the failed `.so.0` attempt |
| `~/pa_callfirst.c` | one **direct compiled call** `pa_strerror(0)` before the dlopen path | **fails** | a single direct call as a warm-up |
| `~/pa_b3` | working baseline re-compiled with `-ldl -Wl,--no-as-needed` | **works** | link flags |
| `~/pa_m2` | hand-rolled sample-spec struct (`{5,48000,2}`) memcpy'd into the header type | **works** | sample-spec layout/type |
| init trace (`LD_DEBUG=all`) | working vs failing binary | **identical** loader output (constructors found, same symbol searches) | load-order/constructor differences |
| ELF shape | both classes: `BIND_NOW`, `NOW PIE`, `RUNPATH=$PREFIX/lib` | identical tags | BIND_NOW / PIE |

Ruled out overall: sample spec + layout, symbol resolution, dlopen order, the
missing `libpulse-simple.so.0` symlink, DT_NEEDED presence, UND refs (versioned
or not), BIND_NOW/PIE, link flags, loader init order, and the daemon path.

What still separates the two classes: binaries built-time-linked **against the
real Termux libs and making at least one direct compiled `pa_*` call** work;
every variant lacking that fails. (Note: `pa_callfirst` is not a counterexample —
`--as-needed` dropped `libpulse-simple.so` from its NEEDED list, so it belongs
to the unlinked class.)

## 5. Fix plan

### 5.1 One decisive experiment (cheap, on-device C)

Compile the `pa_linkorder.c` body (dlsym-only, zero direct `pa_*` calls) **linked
against the real libs with `--no-as-needed`**:
```
clang -o pa_nodirect pa_linkorder.c -ldl -lpulse-simple -lpulse -Wl,--no-as-needed \
      -I/data/data/com.termux/files/usr/include
```
- If it **works** ⇒ the trigger is real linkage against the actual library
  (something about the stub's ELF/absence differs). Rust can then reproduce it
  on CI only by shipping real Android libpulse — impractical — so go to §5.3.
- If it **fails** ⇒ the trigger is having ≥1 direct compiled `pa_*` call
  resolved at startup. Rust can reproduce that: declare `extern "C"` stubs, add
  `-C link-arg=-lpulse-simple` against a stub .so in `build.rs`, and make one
  trivial direct call at backend init.

Also worth probing: AAudio from a console binary (`~/aaudio_probe.c` with the
NDK aaudio headers, `libaaudio.so` is present) — if `AAudio_createStreamBuilder`
works there, the Rust AAudio path's null-builder has a fixable cause.

### 5.2 Keep the shipped improvements

Regardless of outcome: the `android.rs:219` Err-logging, the `.so`-first soname
order, and the CI `patchelf` step are all correct, cheap, and should stay.

### 5.3 Pragmatic fallback that needs no native linking (recommended if §5.1
inconclusive)

Replace the dlopen'd libpulse backend with a **child `pacat` process**:
spawn `pacat --raw --format=float32le --rate=48000 --channels=2` and write the
mixed PCM to its stdin. Advantages:
- `pacat` is a Termux binary that demonstrably streams to the working sink.
- No native linkage, no ABI assumptions, survives PulseAudio API changes.
- Volume control already exists: `engine.rs::set_pipewire_sink_input_volume`
  drives `pactl set-sink-input-volume`, so software gain can drop to unity.
Costs: an extra process per stream, latency of one pipe, and lifecycle
handling (kill child on stop/drop). Keep the direct-FFI path as an optional
feature flag for devices where it works.

## 6. Build/install/test loop (release required)

- The workflow has **no `push` trigger** — only `pull_request` and
  `workflow_dispatch`. Trigger builds explicitly:
  ```
  ssh pipelinepro 'gh workflow run "Build Release Binaries" \
    --repo tim-projects/l337-audio-server --ref main'
  ssh pipelinepro 'gh run list --repo tim-projects/l337-audio-server --limit 3'
  ```
- `install.sh --pre-release` fetches the installer **script and asset from the
  release tag**, so locally fixed scripts/binaries only take effect after a
  release that contains them.
- Releases so far: `2026.10.25-1` (this fix) ← **installed on device**,
  `2026.10.24-1` (installer fix, commit `82a553c`), `2026.10.23-1`.

Foreground debug run (needs `LD_LIBRARY_PATH`):
```
sv down l337-audio-server; pkill -f l337-audio-server; sleep 1
env LD_LIBRARY_PATH="$PREFIX/lib" TMPDIR="$TMPDIR" \
  "$HOME/.local/share/l337-player/bin/l337-audio-server" --transport=socket
```

## 7. Device state to restore (housekeeping)

- Service is **down** (taken down for the `dummy=false` foreground test).
  Restore with `sv up l337-audio-server`; `server.ini` already has
  `dummy = true`, so it will come up healthy.
- The debug PA daemon (pid 18996, `-n --log-level=debug`) is still running and
  writing `~/padebug.log`. Kill it and let the normal service daemon
  (`pulseaudio --start --exit-idle-time=-1`) take over when finished:
  `pkill pulseaudio; pulseaudio --start --exit-idle-time=-1`.
- The installed binary is the official `2026.10.25-1` release (md5
  `4051d2f3…`) — the older `*.pre-patch` manual patch is stale; delete it.
- `patchelf` was `pkg install`ed on-device for these tests — uninstall if
  undesired (CI does its own patchelf on the runner).

Probe artifacts (all in `$HOME` or `$TMPDIR`, outside the repo — do not commit):
`pa_probe.c`, `pa_probe2.c`, `pa_probe3.c`, `pa_ctx_probe.c`,
`pa_stream_probe.c`, `pa_stream2.c`, `pa_deep.c`, `pa_linked.c`, `pa_exact.c`,
`pa_m1.c`, `pa_m2.c`, `pa_b3.c`, `pa_order.c`, `pa_linkorder.c`,
`pa_patcheltest.c`, `pa_stubtest.c`, `pa_dladdr.c`, `pa_callfirst.c`,
`pa_dtneeded_test.c`, `dlopen_probe.c`, `stub_pulse.c`, `stubdir/`, `stubver/`,
`ver.map`, `test440.wav`, `padebug.log`, `watch-ci.sh`.
