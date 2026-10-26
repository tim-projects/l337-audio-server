# Android / Termux audio — plan and findings

Status: **bionic is the correct (only) Termux build. No glibc build is needed.**
**Root cause found and fixed.** The Termux build's `pa_simple_new` returns
`PA_ERR_INVALID` inside our process, while the equivalent documented
`pa_threaded_mainloop`/`pa_context`/`pa_stream` flow succeeds in the *same*
process (proved with an LD_PRELOAD reimplementation: context READY, stream
READY, "Using PulseAudio backend for Android"). The backend now drives that
public API directly, linked (not dlopen'd).

## 1. Confirmed: bionic only, glibc not required

Evidence, gathered on this device (not inferred):

1. `/system/bin/linker64` is the dynamic linker for **both** the server binary
   (`$PREFIX/../.local/share/l337-player/bin/l337-audio-server`) and `pactl`.
   Both are bionic ELFs.
2. `/system/lib64/libc.so` (Android bionic libc) **defines** the
   `.gnu.version_d` symbol versions `LIBC`, `LIBC_N` … `LIBC_V`. Therefore the
   `.gnu.version_r` "LIBC v3/5/6" needs found in `libpulse.so` are standard
   NDK/bionic packaging, **not** a glibc marker.
3. `$PREFIX/lib/libpulse.so` NEEDED list is bionic-only (`libc.so`, `libdl.so`,
   `libdbus-1.so`, `libpulsecommon-17.0.so`). Single package `pulseaudio:17.0-5`;
   no glibc duplicate on the device.
4. `pactl` links that same libpulse and connects to the daemon successfully;
   daemon `/proc/<pid>/maps` shows bionic `/apex/.../lib64/bionic/libc.so`.
5. The `ldd …/usr/glibc/lib/libc.so: invalid ELF header` line is a `ldd`
   wrapper search-order artifact (Termux's `ldd` is itself a glibc-prefix
   helper), **not** evidence about libpulse's ABI.

⇒ glibc is not required. Build matrix keeps `native` (bionic, AAudio only) and
`termux` (bionic + PulseAudio).

## 2. Build configuration (current)

- `Cargo.toml`: `pulseaudio = ["backend"]`.
- `build.yml` (`build-android`): matrix `[native, termux]`,
  `aarch64-linux-android`, NDK r28b, `aarch64-linux-android29-clang`; termux
  builds `--features pulseaudio` → `l337-audio-server-aarch64-android-termux`.
- `src/platform/{mod.rs,android.rs,engine.rs,pulseaudio.rs}`:
  - `android.rs::start_stream` tries PulseAudio first, falls back to AAudio,
    and **logs the real `Err` string**.
  - `pulseaudio.rs` now uses **direct `extern "C"` calls** (no dlopen).
- `build.rs`: for `target_os = "android"` + `CARGO_FEATURE_PULSEAUDIO`, links
  the stubs in `scripts/pulse-stubs` (see §4.7).
- `src/platform/mod.rs`: `pulseaudio` module is gated
  `#[cfg(all(target_os = "android", feature = "pulseaudio"))]` — native/host
  builds never compile it.

No other platform builds are affected.

## 3. Installer (fixed, verified end-to-end)

`scripts/install-android-termux.sh` (commit `82a553c`), manually tested three
times on device:
- stops every running instance before deploying (`sv down` → `pkill` → 10s wait
  → fail loudly with PIDs) — prevents the stale-socket crash-loop symptom.
- waits for `supervise/ok`, `sv up`, verifies `run:`, links the socket.
- run script exports `LD_LIBRARY_PATH="$PREFIX/lib"` (mandatory now).
- requirement checks include pulseaudio installed + daemon running.

## 4. PulseAudio failure: evidence chain

### 4.1 dlopen path fails, spec-independently
`~/pa_probe2.c`: format ∈ {S16LE, S32LE, FLOAT32LE} × rate ∈ {8000…96000} × 2ch
→ **all 15 combos NULL** with `pa_strerror` = "Invalid argument"
(`PA_ERR_INVALID` = 3). Independent of `PULSE_SERVER`, `PULSE_COOKIE`, and
`PULSE_LOG` settings.

### 4.2 The failing layer is `pa_stream_new`
`~/pa_stream2.c`: with a fully READY context, every `pa_stream_new`
argument combination returns NULL.

### 4.3 `pa_context_*` and the daemon are fine
`~/pa_ctx_probe.c`: `pa_mainloop_new` OK → `pa_context_new` OK →
`pa_context_connect` → 0.

### 4.4 The failing client never reaches the daemon
Daemon debug log unchanged (817 lines) across failing probes; `pactl`/`paplay`
connections log fully. ⇒ client-side failure before any native-protocol
connection.

### 4.5 Audio output itself works (decoupled proof)
`paplay ~/test440.wav` → exit 0; daemon log confirms drain, sink RUNNING → IDLE,
sink-input created/freed. The daemon, sink (`OpenSL_ES_sink`) and playback path
are healthy.

### 4.6 ROOT CAUSE (narrowed): dlopen vs. build-time linkage in C
| call path | result |
|---|---|
| `dlopen` + dlsym `pa_simple_new` | NULL, PA_ERR_INVALID |
| binary **build-time-linked** (`-lpulse-simple -lpulse`), direct calls (`~/pa_linked.c`) | `pa_simple_new` **OK** |
| linked binary, calls routed **through dlsym** (`~/pa_exact.c`) | `pa_simple_new` **OK** |

Supporting bisect (all C, bionic, on-device, same daemon):
| variant | direct `pa_*` compiled call? | result |
|---|---|---|
| `~/pa_v2.c` — real-linked, **zero** direct calls | no | **FAILS** |
| `~/pa_v5.c` — drops `pa_simple_free` (keeps `pa_strerror`) | yes | **WORKS** |
| `~/pa_v6.c` — drops `pa_strerror` (keeps `pa_simple_free`) | yes | **WORKS** |
| `~/pa_linkorder.c` — order of `.so.0`-then-`.so` vs `.so` only | both tried | both fail/succeed identically (order irrelevant) |
| `~/pa_b3` — baseline with `-ldl --no-as-needed` | yes | **WORKS** |
| `~/pa_m2` — hand-rolled sample spec `{5,48000,2}` | yes | **WORKS** |

Ruled out: dlopen order, `.so.0` symlink absence, sample-spec layout, link
flags (`--no-as-needed`), thread (`pthread` worker vs main), `libaaudio`
load-order, versioned stub linkage, `BIND_NOW`/PIE, and loader init order.

### 4.7 The Rust rewrite and the stub-link approach (what was built)

`pa_T2.c` — a binary with **no build-time libpulse link**, linked against
committed stubs and calling everything directly — works **fully**:
```
pa_strerror(0)=OK
pa_simple_new -> OK (err=0)
pa_simple_write -> 0
pa_simple_drain -> 0
pa_simple_free OK  ===> FULL PLAYBACK PATH WORKS
```
The stubs (`scripts/pulse-stubs/{libpulse-simple.so,libpulse.so,bionic aarch64,
SONAMEs + `PULSE_0` version script matching the real libs`) exist only at link
time; the real `$PREFIX/lib` libs resolve at runtime.

The Rust implementation now mirrors that exactly:
- `pulseaudio.rs`: `unsafe extern "C"` declarations, direct calls, sample-spec
  struct with the documented field order.
- `build.rs`: emits `cargo:rustc-link-search` + `-l dylib=pulse-simple/pulse`
  for the termux variant only.
- `.gitignore`: `!scripts/pulse-stubs/*.so` (deliberate exception).

### 4.8 Release `2026.10.25-3` still fails (current blocker)

CI (run `40195213611`, triggered via `ssh pipelinepro gh workflow run`) built
both android variants and published `2026.10.25-3`. The termux binary has the
correct NEEDED list:
```
libaaudio.so  libpulse-simple.so  libpulse.so  libdl.so  libm.so  libc.so
```
and verneed `libpulse-simple.so PULSE_0`, `libpulse.so PULSE_0`. On device
(md5 `19136349fb052946b99f40c2df25bf5a`):

| run | result |
|---|---|
| `dummy=true` | starts healthy, dummy mode (regression check OK — no CANNOT LINK) |
| `dummy=false` | `pa_simple_new failed: Invalid argument` → AAudio fallback → null → exit(1) |

So DT_NEEDED + correct verneed + direct `extern "C"` calls is **still not
enough** in the real Rust binary, although the identical C program works.

### 4.9 LD_PRELOAD shim proof: the Rust FFI arguments are correct

`~/pa_shim.c` (LD_PRELOAD interposer) captured the server's actual call:
```
server=(null) name=l337-audio-server dir=0 dev=(null) stream=Audio Output
ss=0x7feb90de08 map=0x0 attr=0x0 err_out=0x7feb90de6c
spec: format=5 rate=48000 channels=2 (size=12)
-> result=0x0 error=3   (PA_ERR_INVALID / "Invalid argument")
```
This eliminates every FFI-side theory: struct layout (12 bytes, fields right),
all values valid (`pa_sample_spec_valid` accepts 5/48000/2 — read from PA 17.0
source), all pointers sane, `dir=0` = `PA_STREAM_PLAYBACK`. Note the interposed
shim's own call **also** fails, so the difference is not the caller's linkage —
it is something about the *process* in which libpulse is driven.

### 4.10 THE FIX: pa_simple_new is Termux-patched; drive the public API

Decisive experiment (`~/pa_myimpl.c`, an LD_PRELOAD shim that **replaces**
`pa_simple_new` with a hand-written sequence using only `libpulse.so`'s public
API, logging every step). First attempt deadlocked inside
`pa_threaded_mainloop_signal`, which exposed two facts:

1. `pa_threaded_mainloop_signal` takes **two** arguments
   (`m, wait_for_accept`) — see `thread-mainloop.h:294` and the device header.
   A one-argument call passes register garbage as `wait_for_accept` and blocks
   forever in `pa_threaded_mainloop_accept`.
2. PA only signals the mainloop for **terminal** states
   (`context_state_cb`/`stream_state_cb` in `simple.c`).

With both corrected, the reimplementation **succeeds inside the Rust process**:
```
1. pa_threaded_mainloop_new      -> 0xb400006f8c00e480
2. pa_threaded_mainloop_get_api  -> 0xb400006f8c015858
3. pa_context_new(name=l337-audio-server) -> 0xb400006f8c059300
4. pa_context_set_state_callback -> ok
5. pa_context_connect(server=(null)) -> 0
6. lock -> ok
7. pa_threaded_mainloop_start -> ok
8. ctx_state: 1 CONNECTING -> 2 AUTHORIZING -> 3 SETTING_NAME -> 4 READY
9. pa_stream_new(stream_name=Audio Output) -> 0xb400006f8c024000
11. pa_stream_connect_playback(dev=(null) flags=7) -> 0
12. stream_state: 1 CREATING -> 2 READY
=== SUCCESS ===
   server log: "Using PulseAudio backend for Android"
```

Conclusion: the failing component is the Termux build of
`libpulse-simple.so` ("17.0-dirty", 300 KB — roughly 10× a stock build — so it
carries patched code). Our Rust implementation now drives the documented
`pa_threaded_mainloop`/`pa_context`/`pa_stream` API directly, which is proven
to work in this very process.

### 4.11 What the fix changed

- `src/platform/pulseaudio.rs`: full rewrite — direct `unsafe extern "C"` calls
  to `pa_threaded_mainloop_*`, `pa_context_*`, `pa_stream_*`; state callbacks
  signal the mainloop only on terminal states with the correct two-argument
  `pa_threaded_mainloop_signal`; `pa_stream_write` per chunk under the mainloop
  lock. No `libloading`, no `pa_simple_*`.
- `scripts/pulse-stubs/`: single stub `libpulse.so` (SONAME + `PULSE_0`
  versions) exporting the 26 symbols above; `libpulse-simple.so` and the
  `strerror-stub*` files are gone.
- `build.rs`: links only `-l dylib=pulse` for the termux variant.
- `build.yml`: the verification step now **fails the job** if `libpulse.so` is
  missing from the NEEDED list (guards the regression).

### 4.12 PA 17.0 source facts (read on-device)

- `pa_simple_new` (`src/pulse/simple.c`) validates server/name/dev/ss/map
  client-side and can only return `PA_ERR_INVALID` (3) from that validation or
  from a context error state; all other failures propagate the context's errno.
- `pa_sample_spec_valid` (`src/pulse/sample.c`): rate ∈ (0, 48000*16],
  channels ∈ (0, 32], format valid — 48000/2/FLOAT32LE are all valid.
- `pa_context_errno(c)` returns `PA_ERR_INVALID` also when `c == NULL`
  (`context.c:1096`) — the only "surprising" source of code 3.
- `pa_context_connect` (`context.c:987`): `!pa_detect_fork()`, state
  `UNCONNECTED`, flags ⊆ {NOAUTOSPAWN, NOFAIL}, `!server || *server`.

Full source downloaded at `$TMPDIR/pa17.tar.gz` for local cross-reference.

## 5. Fix plan

### 5.1 Immediate housekeeping
- **Delete the broken release `2026.10.25-2`** — it was published without the
  termux asset (the `publish-release` job runs with `always()` even when the
  termux build fails). Users hitting `curl: (22) 404` are downloading it.
- Optionally gate `publish-release` on the android jobs when the failure is
  only in `build-android` (the source of that partial release).

### 5.2 Determine why the real Rust process fails where C succeeds (open)
Use the shim approach to interpose the **next** libpulse calls inside
`pa_simple_new`'s state machine — `pa_context_connect`, `pa_context_errno`,
`pa_stream_new`, `pa_threaded_mainloop_start` — and log each step's return in
the real server. That pinpoints which internal step produces `PA_ERR_INVALID`
in the Rust process (e.g. context error object holding INVALID, or the
`c == NULL` branch). `RTLD_NEXT` resolution inside the shim works (already
demonstrated for `pa_simple_new`).
- Do not LD_PRELOAD `timeout`: LD_PRELOAD also applies to the wrapper and
  `timeout` (glibc helper) then fails with `libdl.so` missing. Run the server
  directly (it exits on its own when the backend fails).

### 5.3 Pragmatic fallback that needs no native linkage (recommended)
Replace the PulseAudio FFI backend with a **child `pacat` process**:
spawn `pacat --raw --format=float32le --rate=48000 --channels=2`, write the
mixed PCM to its stdin.
- `pacat` is a Termux binary that demonstrably streams to the working sink.
- No native linkage, no ABI assumptions, survives PulseAudio changes.
- Volume control already exists (`engine.rs::set_pipewire_sink_input_volume`
  via `pactl`), so software gain can drop to unity.
Costs: one extra process per stream, pipe latency, lifecycle handling
(kill child on stop/drop). Keep the direct-FFI path behind a feature flag.

### 5.4 Parallel track: why is AAudio unavailable in a console process?
`AAudio_createStreamBuilder` returns null on Android 16 from a console process.
If AAudio worked, the server would be usable without PulseAudio at all.
Probe `libaaudio.so` directly from a bionic C binary (`~/aaudio_probe.c`) using
the NDK aaudio headers to see whether the failure is library load, builder
creation, or open.

## 6. Build/install/test loop (release required)

- The workflow has **no `push` trigger** — only `pull_request` and
  `workflow_dispatch`. Trigger explicitly:
  ```
  ssh pipelinepro 'gh workflow run "Build Release Binaries" \
    --repo tim-projects/l337-audio-server --ref main'
  ssh pipelinepro 'gh run list --repo tim-projects/l337-audio-server --limit 1'
  ```
- `install.sh --pre-release` fetches the installer **and asset from the release
  tag**, so fixes only take effect after a release that contains them.
- **`gh` is not installed on the phone** — use pipelinepro for all GitHub
  operations. Log download (truncated in `gh run view --log`) works via:
  `gh api -i repos/<r>/actions/jobs/<job_id>/logs` (returns the plain-text log
  on this gh version, not a redirect).
- **Edition-2024 gotcha**: `extern "C"` blocks must be `unsafe extern "C"`
  (hard error; cost one full CI cycle). All `unsafe extern` call sites must be
  wrapped in `unsafe {}`.
- Foreground debug run (needs `LD_LIBRARY_PATH`):
  ```
  env LD_LIBRARY_PATH="$PREFIX/lib" TMPDIR="$TMPDIR" \
    L337__SERVER__DUMMY=false [LD_PRELOAD=/path/shim.so] \
    "$HOME/.local/share/l337-player/bin/l337-audio-server" --transport=socket
  ```

## 7. Device state to restore (housekeeping)

- Service is **down**; `server.ini` still has `dummy = true` → `sv up` to leave
  the phone healthy.
- The debug PA daemon (pid 18996, `-n --log-level=debug`) is still running.
  When finished: `pkill pulseaudio; pulseaudio --start --exit-idle-time=-1`.
- Installed binary: `2026.10.25-3` (md5 `19136349fb…`) — includes the direct
  FFI linkage, which is harmless (dummy mode verified).
- `patchelf` was `pkg install`ed on-device; CI does its own on the runner.
- The stale `*.pre-patch` manual patch backup can be deleted.

Probe artifacts (all in `$HOME` or `$TMPDIR`, outside the repo — do not commit):
`pa_probe.c`, `pa_probe2.c`, `pa_probe3.c`, `pa_ctx_probe.c`,
`pa_stream_probe.c`, `pa_stream2.c`, `pa_deep.c`, `pa_linked.c`, `pa_exact.c`,
`pa_v2.c`, `pa_v5.c`, `pa_v6.c`, `pa_b3.c`, `pa_m2.c`, `pa_order.c`,
`pa_linkorder.c`, `pa_patcheltest.c`, `pa_stubtest.c`, `pa_dladdr.c`,
`pa_callfirst.c`, `pa_T.c`, `pa_T2.c`, `pa_aa.c`, `pa_thread_test.c`,
`pa_shim.c`, `pa_shim.so`, `dlopen_probe.c`, `stub_libpulse.c`, `stub_pulse.c`,
`stubdir/`, `stubver/`, `ver.map`, `ver_pulse.map`, `test440.wav`, `padebug.log`,
`watch-ci.sh`, `pa17.tar.gz`, `simple.c`, `sample.c`, `context.c`.
