# Android / Termux audio — plan and findings

Status: **bionic is the correct (only) Termux build. No glibc build is needed.**

## 0. RESOLVED: PulseAudio audio output works on Termux

Release **`2026.10.26-3`** is installed and **verified producing real audio**:
a 440 Hz test tone played through PulseAudio was captured from the sink
monitor (`pacat -d OpenSL_ES_sink.monitor`), and analysis of the 15.9 s
recording shows peak 0.54 / RMS 0.089 with a dominant **440 Hz** component
(magnitude 0.148) and ~0 at 220/880 Hz — exactly the tone that was played.
The supervised service (`sv up l337-audio-server`) now runs with
`dummy = false` and plays tracks through `OpenSL_ES_sink`.

Two bugs had to be fixed after the linkage work; both were FFI-signature
problems invisible at compile time (§4.13). Full chain of findings below.

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

### 4.13 The two FFI-signature bugs that followed

Verified with an LD_PRELOAD shim (`~/pa_wshim.c`) that interposes
`pa_stream_write`/`pa_stream_get_state` and logs PA's return codes:

**Bug 1 — `pa_threaded_mainloop_signal` takes two arguments.**
`pa_threaded_mainloop_signal(m, wait_for_accept)`. Declaring one argument
passes register garbage as `wait_for_accept`; when non-zero the caller blocks
forever in `pa_threaded_mainloop_accept`. This deadlocked the first
reimplementation attempt. (Confirmed from `thread-mainloop.h:294`.)

**Bug 2 — `pa_stream_write` takes six arguments.**
`(s, data, nbytes, free_cb, offset, seek)` — the 6th is `pa_seek_mode_t`
(`stream.h:519`). Declaring five left garbage in that register, and PA
returned **-3 / `PA_ERR_INVALID`** from the validity check
`seek <= PA_SEEK_RELATIVE_END`. The daemon log showed the stream itself was
created fine (`float32le 2ch 48000Hz`), so the failure was invisible until the
shim logged:
```
[w] pa_stream_write(s=0x... data=0x... nbytes=1024 free_cb=0x0 offset=0) -> begin
[w] pa_stream_write -> -3
```
Fix: pass `PA_SEEK_RELATIVE` (0). Lesson applied to every other declaration —
all signatures were audited against `$PREFIX/include/pulse/*.h`, and the ones
that were already correct (e.g. `pa_stream_connect_playback`, 6 args) were
re-confirmed by the shim observing them succeed.

Also fixed along the way: the writer loop must not sleep while holding the
mainloop lock (the PA mainloop thread needs it), and the `handle` MutexGuard
may not be destructured before use (E0382, caught by CI).

### 4.14 CI pitfalls hit

- **Edition 2024 requires `unsafe extern "C"`** (hard error).
- **Release version auto-increments.** When a publish job runs after a tag
  already exists, the new release is `2026.10.26-2`, not `-1` — an artifact
  download can 404 if you request the earlier tag. A failed build still
  publishes (job has `always()` + `continue-on-error`), producing a partial
  release; the *next* successful run publishes the complete one at `-N+1`.
- `gh` is not installed on the phone; all GitHub operations go through
  `ssh pipelinepro`. Job logs come from
  `gh api -i repos/<r>/actions/jobs/<id>/logs` (plain text on this gh).

### 4.15 Verification method (reproducible)

```
sv down l337-audio-server
env LD_LIBRARY_PATH="$PREFIX/lib" TMPDIR="$TMPDIR" \
    L337__SERVER__DUMMY=false \
    ~/.local/share/l337-player/bin/l337-audio-server --transport=socket
# in another shell: record the sink while playing a file:// track
timeout 8 pacat --record --raw --format=float32le --rate=48000 --channels=2 \
    -d OpenSL_ES_sink.monitor > mon.raw
# then analyse mon.raw for a non-zero RMS and the expected dominant frequency
```

### 4.16 PA 17.0 source facts (read on-device)

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

## 5. Fix plan (complete)

Done, in order:

1. **Bisect the C failure** (dlopen vs. build-time linkage) — §4.6/§4.8.
2. **Ship stubs + build.rs** so the binary is linked, not dlopen'd.
3. **Replace `pa_simple_*` with the public `libpulse.so` API** — the Termux
   `pa_simple_new` is patched and fails in our process (§4.10).
4. **Fix the two argument-count bugs** found by LD_PRELOAD shims (§4.13):
   `pa_threaded_mainloop_signal` (2 args) and `pa_stream_write` (6 args).
5. **Audit every declaration against the headers** — the recurring lesson.
6. **Verify real output** by recording the sink monitor (§4.15).

Not needed / rejected:
- `pacat` child-process fallback (§5.3 of the old plan) — unnecessary now.
- Housekeeping of the pre-fix `2026.10.24-1` / broken `2026.10.25-2`
  releases left as-is.

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
