# plan-linux.md — Linux-specific code-change plan for l337-audio-server

Generated from the `2026-11-05` Arch Linux (PipeWire) debugging session recorded in
`bugs.md` ("Session — 2026-11-05" section + consolidated table rows 1–7). Each item is a
self-contained prompt an implementing agent can execute against the codebase. Items are
ordered by risk/impact if left unaddressed.

Host used for verification: Arch Linux (7.3.1-1-cachyos, x86_64), PipeWire active, real ALSA
sink `alsa_output.pci-...__Speaker__sink` RUNNING via PipeWire's ALSA plugin. Built locally
with `--no-default-features --features alsa,opus` (pipewire.pc/pulseaudio.pc unavailable, so
the pulseaudio backend could NOT be built here). No passwordless sudo, no `gh`, no `yt-dlp`
initially (fetched a standalone yt-dlp binary for the YouTube steps).

---

## A. Code-change prompts (modify source / tests / CI)

### A1. Merge `g1-opop-decode` → `main` (blocking, highest impact)
**Context:** All fixes — Opus decode, BUG-1..BUG-4, the wedged-MKV reader rebuild, the G3
streaming default — are committed on `g1-opus-decode` but invisible to users until it lands.
The monthly runner cuts official releases only from `main`. bugs.md table row 9.

**Prompt:**
> Merge the `g1-opus-decode` branch into `main`. Resolve any conflicts. After the merge,
> confirm `main` contains: `AudioBuffer::consumed_total` (BUG-1), the bounded-window +
> `reclaim_consumed()` writers (BUG-2), `get_status` end-of-track `stopped` (BUG-3),
> highest-bitrate format selection (BUG-4/G2), `wait_for_progress` shared helper (Finding
> #10), the wedged-MKV `reprobe_format()` reader rebuild (Part 7 U2/U7), and the G3
> streaming-default strategy (probe → fallback to whole-file download). Run `cargo check
> --no-default-features --features alsa,opus` (CARGO_HOME/CARGO_TARGET_DIR in /tmp per
> AGENTS.md) to confirm the tree compiles. Do not cut a release from this merge in this
> prompt.

### A2. Add a CI `cargo test` job (regression safety)
**Context:** No CI workflow runs `cargo test` — only `cargo check`/`build`. The host run
caught a real bug (`decode_to_pcm` returns a 4-tuple; tests destructured a 3-tuple) that
`cargo build`-only CI never saw. bugs.md table row 8.

**Prompt:**
> Add a `cargo test` job to `.github/workflows/build.yml` (or a dedicated workflow) that runs
> the full suite on Linux:
> `cargo test --no-default-features --features alsa,opus` (and, if pipewire dev headers are
> available in the runner, `--features pipewire,opus` too). Pin `CARGO_HOME`/`CARGO_TARGET_DIR`
> to /tmp per AGENTS.md to avoid disk exhaustion. Require the job to pass before any release
> dispatch. Confirm locally first with:
> `CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build cargo test --no-default-features --features alsa,opus`

### A3. Torn-cluster WebM fixture + CI regression test (table row 1)
**Context:** The 2026-11-05 re-probe fix for the wedged MKV reader is verified on a live
YouTube track (`dQw4w9WgXcQ`, froze at 9 s in `2026.11.03-1`; now advances 19→30 continuous).
But there is no in-repo WebM fixture with a deliberately torn cluster, so a reworded symphonia
error message or resequenced demuxer would silently lose the immediate rebuild path and fall
back to the ~5 s generic path. bugs.md caveats under "WebM/Opus streaming stalls at ~9 s".

**Prompt:**
> Create an in-repo WebM/Opus fixture containing a deliberately torn cluster (truncated
> SimpleBlock / element whose data crosses a snapshot length), and add an integration test
> that drives `streaming_decode_sync` against it with a growing download file, asserting the
> reader rebuilds on the `past master element` wedge and `position_sec` advances past the old
> freeze point with no rewind. Place the fixture under `tests/fixtures/` (or similar) and gate
> it in CI so a symphonia upgrade that rewords the error message fails loudly. Reference:
> `src/player/engine.rs::reprobe_format()`, the Opus/Symphonia/raw-packet streaming loops, and
> the `wedge_action()` / `rebuild_due` policy.

### A4. Incremental file decode (Finding #3 shortcut, table row 10)
**Context:** The file/`/player/play/stream` path still decodes the whole track into `buf.pcm`
in one shot (`engine.rs:522`), pinning ≈1.1 GiB for a 60-min stereo @48 kHz track. The
documented shortcut keeps this and marks "full incremental file decode" as the upgrade path.
bugs.md Part 1 Finding #3 and table row 10.

**Prompt:**
> Decode local files incrementally into the throttled `audio_buffer` producer instead of
> assigning the whole decoded track at once. Reuse the existing "disk file"
> `MediaSourceStream` + streaming packet loop (`streaming_decode_sync` /
> `throttle_stream_producer`) so the file path becomes length-independent like the streaming
> path. At minimum: pre-reserve/shrink `decode_to_pcm` so capacity tracks the unconsumed
> window rather than track length. Update the `load_and_play` shortcut comment and the RSS
> reclaim verification (table row 3) to confirm the whole-track peak is eliminated.

### A5. Real bit-exact enforcement / lossless preference (Finding #6, optional future)
**Context:** `Quality::Lossless` maps to ceiling 0 (no ceiling) like `best`; nothing calls
`needs_bitexact()` (removed). `PlayerStatus.codec` reports the actual delivered encoding but
there is no lossless-format preference or degradation flag. A lossless request silently
returns lossy audio. bugs.md Part 1 item 6 / table row 6 (in the original finding list).

**Prompt:**
> Wire `needs_bitexact()` (or an equivalent) into `yt_dlp_format_selector` so a `lossless`
> request prefers a lossless format and returns an explicit degradation flag when none exists.
> Update `PlayerStatus` to expose the degradation flag, and keep `codec` reporting the actual
> delivered encoding. Mark as optional/future — the server already never re-encodes, so the
> core security property holds; this closes the overclaim gap.

---

## B. Verification prompts (no code change; run on a real Linux+PipeWire host)

### B1. pulseaudio backend `CHUNK_FRAMES=4096` device re-check (table row 7)
**Context:** `CHUNK_FRAMES` raised 256 → 4096 in `src/platform/pulseaudio.rs`. Not executed here
because `pipewire.pc`/`pulseaudio.pc` are absent on this Arch host, so the pulseaudio backend
couldn't be built.

**Prompt:**
> On a host with pipewire/pulse dev headers, build `--features pulseaudio,opus`, run the
> isolated-instance recipe (port 13370, real PipeWire sink), and confirm: audio at 1×, no
> underrun/overrun, lower CPU/wakeups (rough `top` check), and `position_sec` still advances
> ~1×. Compare wakeup cadence before/after the CHUNK_FRAMES change if a baseline is available.

### B2. Install-from-scratch on a host without libopus (Finding #1, table row 6)
**Context:** `readelf -d`/`ldd` clean here; installer dependency-check gate verified in code
(`ensure_runtime_dependencies()` fails loudly on unresolvable NEEDED). But the apt/dnf/pacman
install invocation needs root — not executable here (no passwordless sudo).

**Prompt:**
> On a stock Debian/Fedora/Arch host *without* libopus: run the Linux installer
> (`install-linux-pipewire.sh` / `install-linux-alsa.sh` / `install-systemd.sh`), expect it to
> detect the missing `libopus.so.0`/`libopus0`, install it via the package manager, re-check
> with `ldd`/`readelf -d`, and report `Runtime libraries present: …`. Confirm the service
> starts (`systemctl --user status l337-audio-server` running, no `CANNOT LINK`). Also confirm
> a dummy-mode boot (`L337__SERVER__DUMMY=1 … --version`) works, and that the installer does
> not silently succeed when a needed lib is missing.

### B3. Mid-stream seek then wedge (table row 1)
**Context:** The live-wedge fix is verified on a cold track. Seek-in-stream through the same
rebuild path is untested.

**Prompt:**
> On a live PipeWire host, start a YouTube Opus track, seek mid-stream (`POST /player/seek`),
> then let it hit the MKV wedge point; confirm `position_sec` continues advancing after the
> seek-with-rebuild with no rewind or stall. Reference `seek_and_resume()` and the streaming
> loops in `src/player/engine.rs`.

### B4. Unprobeable-container fallback + `STREAM_PROBE_BUDGET` tuning (table row 2)
**Context:** G3 streaming default confirmed fast on probeable containers. The fallback path
(whole-file download for containers that cannot be probed from a partial file) and the 35 s
budget are not tuned against a real log.

**Prompt:**
> Find/construct a container that genuinely cannot be probed from a partial file (e.g. a
> progressive MP4 with `moov` at the end). Confirm `POST /player/play` falls back to whole-file
> download and still plays, and tune `STREAM_PROBE_BUDGET` (currently 35 s) against a real log
> so genuine unprobeable containers are observed rather than timed out blind. Reference
> `src/player/engine.rs::streaming_decode_sync` probe outcome / `probe_result` oneshot.

### B5. BUG-4 AAC direction (table row 5, one direction verified)
**Context:** Opus max → Opus 129 k verified on `dQw4w9WgXcQ`. The AAC-max direction
(AAC 140 → AAC 129 k) was verified 2026-11-02 on a different device but not re-checked here.

**Prompt:**
> Re-verify BUG-4 both directions on a live host with `yt-dlp -F`: confirm `best`/`target_kbps=0`
> selects the highest-bitrate audio rendition at or below the ceiling — AAC 129 k where AAC 140
> is the max, Opus 144 k where itag 251 is the max — reported via `PlayerStatus.actual_kbps`/
> `codec`. Use a fake `yt-dlp` recording the requested format id for unit/endpoint regression
> coverage.

---

## C. Reproduction context (isolated-instance recipe used on this host)

Recreate the exact isolated instance:

```bash
mkdir -p /tmp/l337iso/{data,run,state,cache,logs} && chmod 700 /tmp/l337iso/run
# fixtures (ffmpeg required; libopus for .opus/.webm)
cd /tmp/l337iso/data
ffmpeg -f lavfi -i "sine=frequency=440:duration=10:sample_rate=48000" -ac 2 -c:a libopus test_opus_ogg.opus -y
ffmpeg -f lavfi -i "sine=frequency=330:duration=10:sample_rate=48000" -ac 2 -c:a libopus test_opus_webm.webm -y
ffmpeg -f lavfi -i "sine=frequency=220:duration=100:sample_rate=48000" -ac 2 -c:a libopus long_opus_ogg.opus -y

# server (CARGO_HOME/CARGO_TARGET_DIR in /tmp per AGENTS.md)
export CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build RUSTUP_HOME=/home/tim/.rustup
export PATH="$CARGO_HOME/bin:$PATH"
env L337__SERVER__TRANSPORT=http L337__SERVER__HOST=127.0.0.1 L337__SERVER__PORT=13370 \
    L337__SERVER__DEFAULT_TARGET_KBPS=128 L337__SERVER__BUFFER_MAX_BYTES=8388608 \
    L337__SERVER__TOKEN=dbgtkn123 RUST_LOG=l337_audio_server=debug \
    HOME=/tmp/l337iso XDG_RUNTIME_DIR=/tmp/l337iso/run \
    XDG_CACHE_HOME=/tmp/l337iso/cache STATE_DIRECTORY=/tmp/l337iso/state TMPDIR=/tmp/l337iso \
    /tmp/l337-build/release/l337-audio-server

BASE=https://127.0.0.1:13370; TOKEN=dbgtkn123
q() { curl -sk --connect-timeout 5 -H "Authorization: Bearer $TOKEN" "$@"; }
```

Representative checks:
- §1 position advance: `q -X POST $BASE/player/play/stream -H 'X-Track-Id: pos1' --data-binary @...test_opus_ogg.opus`; poll `$BASE/player/status | jq '{position_sec,duration_sec,state,codec}'`
- §2 RSS reclaim: `grep VmRSS /proc/<pid>/status` before/playing/after-stop on the 100 s track
- §4 ceiling: `q -X POST $BASE/player/target -d '{"target_kbps":64}'`; play YouTube; assert `actual_kbps<=64`
- Table row 1: play `https://www.youtube.com/watch?v=dQw4w9WgXcQ` (Opus/251); confirm `position_sec` passes 9 s continuously

---

## D. New bugs found in comprehensive test battery (fix prompts)

These came from running a full 21-endpoint battery against the alsa+opus binary on
this Arch+PipeWire host (recorded in bugs.md "New findings — comprehensive API test
battery").

### B1. NF-1 — Align seek request/response field names (`position` vs `position_sec`)

**Context:** `SeekPayload.position` (src/api/models.rs:125) but `PlayerStatus.position_sec`
(src/api/models.rs:153). Sending `position_sec` to seek yields 422 "missing field `position`".

**Prompt:**
> Align the field name used by the seek request and the status response. Either rename
> `SeekPayload.position` to `SeekPayload.position_sec` (and update `engine.seek()` and
> `handlers.rs::seek` accordingly) or rename `PlayerStatus.position_sec` to
> `PlayerStatus.position`. Update all call sites, tests, and documentation. Verify with
> `POST /player/seek {"position_sec":5}` returning 200 (or `{"position":5}` returning 200
> if keeping `position`).

### B2. NF-2 — Return JSON `{"ok":true}` bodies on all mutation endpoints

**Context:** `upload_stream` (handlers.rs:493), `set_volume` (handlers.rs:346), `set_speed`
(handlers.rs:336), `set_target`, `stop`, `pause`, and `settings` return empty 200 OK bodies.
Only `/player/play` returns `{"ok":true}`.

**Prompt:**
> Make all POST/PUT mutation endpoints return `Json(json!({"ok": true}))` with `StatusCode::OK`
> instead of `StatusCode::OK.into_response()` with an empty body. Affected handlers:
> `upload_stream`, `set_volume`, `set_speed`, `set_target`, `pause`, `stop`, `set_settings`.
> Keep error paths returning appropriate status codes + error bodies. Verify with curl that
> each mutation endpoint returns `{"ok":true}`.

### B3. NF-3 — Document silent clamping for out-of-range volume/speed (design note)

**Context:** `set_volume` clamps to [0.0, 1.0] (engine.rs:850), `set_speed_and_pitch` clamps
to [0.25, 4.0] (engine.rs:832-833). Out-of-range values return 200 with clamped value, not 400.

**Prompt:**
> Document in the API reference (or handler doc comments) that volume is clamped to [0,1] and
> speed/pitch to [0.25,4.0] rather than rejected. No code change required unless the product
> decides to return 400 for out-of-range values instead.

---

## E. Constraints / what could NOT be done on this host

- pulseaudio-backend build & `CHUNK_FRAMES` device re-check: no `pipewire.pc`/`pulseaudio.pc`
- try_recover via real sink disturbance: alsa writes to `plughw:0,5` directly; sink suspend did not raise a backend_error
- installer install-from-scratch (apt/dnf/pacman): no passwordless sudo
- release dispatch / tag creation: no `gh`, no GitHub write from this session
- `cargo test` in CI: no CI job exists yet (see A2)

Items marked **PASS** in bugs.md table rows 1–5 need no further work; rows 6–7 are PARTIAL/NOT-EXECUTED as noted above.