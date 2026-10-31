# Playback Compatibility & Bandwidth Plan

## Objective

Make the audio server (1) able to play back **all audio formats** it will actually be handed —
including every audio format YouTube serves — and (2) expose a **source‑agnostic bandwidth
control**: a numeric **target bandwidth** (`target_kbps`, a *ceiling* in kilobits/sec) as the
primitive, plus an **agreed ladder of human‑labeled quality presets** mapped onto it, ranging from
~1 kbps (speech/podcasts) up to a full **32‑bit lossless** stream. The presets are a convenience
layer for frontends; the server‑authoritative primitive is the kbps number. Default = **best**
(no ceiling; bit‑exact pass‑through when the source is lossless).

This plan also captures the engine‑level defects found while diagnosing "the app shows *Playing* but
no audio is heard", and organizes every fix as **Global (all platforms)** then **Termux/Android**.

## Current Status / Evidence (device: Termux/Android, runit‑managed, PulseAudio via OpenSL_ES)

Playing `https://www.youtube.com/watch?v=bZiCchjRW_s` ("I Need Your Lovin'", `N‑R‑G ‑ Topic`)
returns `state=playing` with `audio_available=true`, `duration_sec=null`,
`position_sec=0` forever, and **no sound**. Server logs (RUST_LOG=debug) show:

- Compiled codecs: `mp3, aac, adpcm, pcm, vorbis, flac`. Compiled demuxers: `isomp4, mkv, ogg, riff`.
  `Cargo.lock` has **no** `symphonia-codec-opus`, `symphonia-codec-alac`, `libopus`, or `opus`.
- `play_track: YouTube streaming for <page-url>` → the server's resolvers
  (`resolve_youtube_stream_url`, `download_via_ytdlp` in `src/player/engine.rs`) select
  `-f bestaudio`, which YouTube serves as **Opus in WebM (Matroska)** (itag 251, `mime=audio/webm`).
- `streaming_decode_sync: selected track codec params … extra_data = [79,112,117,115,72,101,97,100 …]`
  = ASCII `"OpusHead"` → CodecType 4101 (Opus).
- `streaming_decode_sync: decoder creation failed … Unsupported("core (codec):unsupported codec")`
  → `streaming decode task failed: decoder init: unsupported feature: core (codec):unsupported codec`.
- Feeding the server an already‑downloaded **AAC/m4a** file via `POST /player/play/stream`
  (`load_and_play`) decodes correctly (`codec CodecType(4100)` = AAC, `load_and_play(current): Playing
  (duration Some(231)s)`), proving the **file path** and the PulseAudio→OpenSL output path work.
- Repeatable **streaming** failure for fMP4 too: pushing an AAC (itag 140) direct URL still goes
  through `start_streaming_playback` → `streaming_decode_sync`, which re‑probes a partially
  downloaded fragmented MP4 until `format probe failed after retries: end of stream`.
- The engine buffers an entire decoded track in one `AudioBuffer::pcm`; the PulseAudio writer
  (`src/platform/pulseaudio.rs::writer_loop`) drains it in a ~1 s burst with no pacing and never
  reclaims consumed samples, and it enforces `pcm.len() > max_bytes/4` (default `8 MiB`,
  `buffer_max_bytes` in `src/main.rs`). Result for a full song: `buffer cap exceeded` →
  backend error → recovery aborts the process
  (`Assertion 'pa_atomic_load(&(s)->_ref) >= 1' failed … pa_stream_disconnect … Aborting`).

---

# Part A — Global fixes (all platforms)

## G1. Decode Opus (and ALAC) — the core "all formats the server will be handed" fix

**Problem.** YouTube's `bestaudio` is Opus; the server cannot decode it, so any track that the
server resolves itself streams nothing. Containers are already covered (`format-mkv` handles
WebM/Matroska, `format-ogg` handles Ogg), so only the **codec** is missing.

**Change.**
- `Cargo.toml`: add `"opus"` to the `symphonia` feature list (pulls `symphonia-codec-opus`).
  Add `"alac"` for full general coverage (not used by YouTube, but completes the matrix and is a
  common lossless source a front‑end may push).
- The `symphonia-codec-opus` crate is a **libopus** wrapper: the build must link libopus (see
  Termux/A1 for the Android/NDK specifics; on desktop CI ensure the `opus`/`libopus` dev package is
  present). No `dlopen` — link at build time, matching how the codebase already links `libpulse`.
- `streaming_decode_sync`'s allowed‑codec filter already lists `CODEC_TYPE_OPUS`; the gap is purely
  the missing decoder registration, so no filter change is needed.
- Capability reporting (`capabilities.yt_dlp`) is unchanged.

**Files.** `Cargo.toml`, `build.rs` (linkage), `src/player/engine.rs` (verify probe/codec paths).

**Test.** Unit: decode a bundled Opus‑in‑WebM buffer through `decode_to_pcm`; assert PCM len > 0,
sample rate/channels sane. Integration: play a known Opus‑only YouTube id; assert `duration_sec`
populated and `position_sec` advances.

**Acceptance.** A YouTube URL resolved server‑side to Opus plays with audible output and an
advancing timeline, no `unsupported codec` in logs.

## G2. Source‑agnostic target‑bandwidth control (default `best`)

**Design.** Bandwidth is a single number: **`target_kbps`, a ceiling in kilobits/sec**. The server
picks the highest‑quality representation of *any* source that fits within that ceiling. It is **not
YouTube‑specific**: it governs rendition selection, lossy re‑encoding, and pass‑through for YouTube,
archive.org/network URLs, SFTP, and local/pushed files alike. `0` / absent means **no ceiling —
`best`** (bit‑exact pass‑through when the source is lossless, else the top rendition).

Human labels are a **stable front‑end convenience** mapped to ceilings (below). Front‑ends may send
the number directly or a label; the server accepts both. This is a *target ceiling*, not adaptive
bitrate.

**Agreed canonical preset ladder** (server advertises this so every front‑end agrees; ceilings are
retunnable per‑era because the labels are the stable contract):

| `quality` label | `target_kbps` ceiling | Practical band (kbps) | Notes / typical use |
|-----------------|-----------------------|-----------------------|---------------------|
| `lowest`        | 16                    | 1–16                  | Ultra‑constrained links, voice/assistants, ~8–12 kbps speech |
| `low`           | 48                    | 16–48                 | Podcasts / talk (12–48 kbps Opus/AAC speech) |
| `medium`        | 128                   | 48–128                | Standard music streaming (typical AAC/Opus) |
| `high`          | 256                   | 128–256               | High‑quality music |
| `veryhigh`      | 512                   | 256–512               | Transparent‑ish music / extra headroom |
| `extreme`       | 1024                  | 512–1024              | Near/hi‑res lossy, heavy headroom |
| `best` (default)| 0 (no ceiling)        | source‑max            | Highest available; bit‑exact pass‑through if lossless |
| `lossless`      | 0 (no ceiling) + require bit‑exact | >1024 … up to 32‑bit lossless | Bit‑exact pass‑through only (FLAC/ALAC/WAV/PCM); never a lossy re‑encode |

Ceiling sanity anchors (why the top is open‑ended): CD audio ≈ 1411 kbps; 24‑bit/96 kHz stereo ≈
4608 kbps; **32‑bit float/48 kHz stereo ≈ 3000 kbps**; 32‑bit float/192 kHz stereo ≈ 12.3 Mbps.
`best`/`lossless` therefore scale to full 32‑bit lossless (and lossless hi‑res) without redefining the
ladder; the lossy tops (`extreme`≈1024 kbps) leave clean headroom above CD while staying under lossless.

**API.**
- `src/api/models.rs` — on `Track`:
  ```rust
  #[serde(default)] pub target_kbps: Option<u32>,   // ceiling; Some(0) and None == best/no ceiling
  #[serde(default)] pub quality: Option<Quality>,   // sugar; enum rename_all="lowercase"
  ```
  Resolution rule in the handler: if `target_kbps` is `Some(n>0)` use it; else if `quality` is set
  map it through the ladder; else use `[server] default_target_kbps`; else `best`. Unknown/absent
  `quality` never errors — it falls back to `best`.
- `src/api/handlers.rs::play` / `::upload_stream` — resolve the ceiling and pass it into the engine.
- Config: `src/main.rs` `[server] default_target_kbps = 0` (best). Optional runtime setter
  `POST /player/target` `{"target_kbps":N}` to change the default without a restart.
- Observability: add `target_kbps`, `actual_kbps`, and `codec` to `PlayerStatus` so a front‑end can
  show "streaming 96 kbps Opus, target 128".

**Realizing the ceiling for each source (`src/player/engine.rs`).** One policy, applied everywhere:
1. **Rendition selection** — pick the highest‑average‑bitrate variant `<= ceiling`. YouTube: build the
   `-f` selector from the ceiling (see table below) at the two `bestaudio` sites and in
   `start_streaming_playback`. Other sources that expose variants: same ceiling test.
2. **Transcode‑down** — if the only/highest variant exceeds the ceiling and a transcoder is present
   (ffmpeg via `Command`), decode→re‑encode to ~ceiling (AAC/Opus at `-b:a <ceiling>k`). This is what
   makes the ceiling apply to **local/pushed files and single‑stream network sources** (e.g. serving a
   local FLAC to a remote client on a metered link).
3. **Pass‑through** — if `ceiling >= source bitrate`, or `best`/`lossless`, stream bit‑exact (no
   re‑encode).
4. **Clamp + report** — if neither selection nor transcode can meet the ceiling, use the lowest
   available and report the true `actual_kbps`; never fail playback solely because a ceiling can't be
   met.

YouTube selector mapping (replaces the hard‑coded `-f bestaudio`; ceiling→`abr<=` bounds, with the
trailing fallback so selection never fails):

| Ceiling (kbps) | yt‑dlp `-f` |
|----------------|-------------|
| ≤ 16           | `bestaudio[abr<=16]/bestaudio` |
| ≤ 48           | `bestaudio[abr<=48]/bestaudio` |
| ≤ 128          | `bestaudio[abr<=128]/bestaudio` |
| ≤ 256          | `bestaudio[abr<=256]/bestaudio` |
| generic `N`  | `bestaudio[abr<=N]/bestaudio` |
| 0 / best    | `bestaudio` |

**Stopgap (pre‑G1):** until Opus decodes, gate `best`/unknown to a decodable codec
(`bestaudio[acodec^=mp4a]/…`) so the server never self‑selects a codec it can't play. Remove once G1
ships; cover with a test.

**Files.** `src/api/models.rs`, `src/api/handlers.rs`, `src/main.rs`, `src/player/engine.rs`.

**Test.** (a) Table test: label→ceiling and ceiling→selector (assert exact strings incl. fallback).
(b) For one set per source class — YouTube (AAC+Opus variants), a local FLAC, a network mp3 — play
each tier; assert `actual_kbps <= ceiling` whenever a variant/transcode exists, that `best`/`lossless`
does **not** transcode a lossless source (bit‑exact), and that `low` picks/encodes a small stream.

**Acceptance.** `{"quality":"low"}` (~12 kbps speech) and `{"target_kbps":300}` (arbitrary front‑end
ceiling) both bound the delivered bitrate across *all* sources, not just YouTube; omitting both plays
best/lossless. `PlayerStatus.actual_kbps` reflects reality on every source.

## G3. fMP4 streaming is unreliable — buffer the init segment or use the file path

**Problem.** `streaming_decode_sync` probes a *growing* fragmented MP4 whose `moov`/init segment
isn't fully present, so Symphonia's `isomp4` probe keeps returning Err until
`format probe failed after retries: end of stream` (observed for an AAC/m4a direct URL).

**Change.**
- Choose the decode strategy by container, not a blind retry loop:
  - **MP4/M4A (fMP4/DASH audio):** download the complete file, then `load_and_play` (the file path
    already decodes reliably — proven for AAC). Incremental streaming isn't valuable here because
    `moov` forces near‑full buffering anyway.
  - **WebM/Matroska (Opus/Vorbis) and Ogg:** keep the incremental `streaming_decode_sync`; these
    cue/index up front and stream cleanly, which is where incremental playback pays off.
- Regardless of container, gate the probe on having enough bytes for a complete init segment
  (don't 60×500 ms re‑open a truncated file).

**Files.** `src/player/engine.rs` (`play_track`, `start_streaming_playback`, `streaming_decode_sync`).

**Test.** Unit with a truncated fMP4 fixture: assert we route to full‑download + `load_and_play`.
Integration: play an AAC/m4a track; assert `duration_sec` set and audio plays.

**Acceptance.** No `format probe failed after retries` for any supported container; AAC and Vorbis
both play.

## G4. Bounded, real‑time‑paced audio window (fixes silent timeline + OOM)

**Problem.** The engine decodes the *entire* track into `AudioBuffer::pcm`; the writer drains it in a
~1 s burst (no pacing) and never reclaims consumed samples; the cap is checked on total `len()`.
Consequences: (a) `buffer cap exceeded` → silence + recovery crash for any track longer than ~20 s
at the 8 MiB default; (b) the reported playhead snaps past the end instantly; (c) RSS is O(track).

**Change.**
- Enforce the cap on the **unconsumed window**, not the whole buffer: `(pcm.len() - read_pos) >
  max_bytes/4`, in every backend (`pulseaudio.rs`, and the equivalents for ALSA/CoreAudio/WASAPI/AAudio).
- Reclaim consumed samples: when the writer hits a checkpoint, `pcm.drain(..read_pos); read_pos = 0`
  (under the existing `Mutex` the FFI callbacks already take).
- Pace the writer to the device clock: after writing `n` frames, sleep ~`n / sample_rate` (or track a
  next‑deadline timestamp) instead of busy‑looping on `available`. This makes `read_pos` advance at
  1× so `calculate_position`/`get_status` report a correct, monotonic timeline.
- Reinterpret `buffer_max_bytes` as the **prebuffer/rebuffer window** (a few seconds of audio),
  not the whole track; document that a whole‑track buffer is neither required nor allowed. This
  window is independent of G2's `target_kbps` (window = rebuffer depth; target = link bitrate).
- Keep the backend‑error path degrading gracefully (see G5).

**Files.** `src/platform/common.rs` (`AudioBuffer` semantics/doc), each `src/platform/*` backend
writer loop, `src/player/engine.rs`.

**Test.** Unit: feed > window bytes; assert drain/reclaim keeps memory bounded and position advances
by ~elapsed time (not instantly). Integration: play a full‑length track; assert `position_sec`
increments ~1/s toward `duration_sec` and RSS stays bounded.

**Acceptance.** Long tracks play to completion; `position_sec` tracks wall‑clock; no
`buffer cap exceeded` under normal load; memory independent of track length.

## G5. Backend‑error recovery must not abort the process

**Problem.** On backend error, `try_recover` tears down and rebuilds the stream; on PulseAudio this
hits `Assertion 'pa_atomic_load(&(s)->_ref) >= 1' failed … pa_stream_disconnect … Aborting`, killing
the server (runit silently restarts it, ending playback).

**Change.**
- Make recovery a clean, ordered state machine: signal the writer/callback to stop, join or fence
  it, *then* disconnect/unref exactly once; never disconnect a stream that is already being dropped
  by `AudioOutputStream::drop`.
- Idempotent teardown: each backend's `stop()`/`Drop` must be safe after a failed `start_stream` and
  safe to call twice.
- If a stream cannot be recovered, surface `state=stopped` + `audio_available=false` to the client
  rather than aborting the process.

**Files.** `src/player/engine.rs` (`try_recover`, `check_backend_error`, `take_backend_error`),
`src/platform/*` (`play`/`pause`/`stop`/`Drop`).

**Test.** Unit: force `backend_error`; assert recovery returns Ok and the process does not abort
(no panic/abort), state transitions are consistent. Stress: repeatedly trip and clear the flag.

**Acceptance.** `try_recover` never aborts; a transient sink error recovers or cleanly reports
`stopped`.

## G6. Sane config defaults (no silent‑by‑default installs)

**Problem.** `ensure_config_file`/`DEFAULT_CONFIG` (`src/main.rs`) writes
`buffer_max_bytes = 8388608` and no bandwidth default; a freshly generated config is silent for
normal tracks under the current whole‑track model.

**Change.**
- `DEFAULT_CONFIG`: `default_target_kbps = 0` (best/lossless), and a `buffer_max_bytes` appropriate
  to the bounded‑window model from G4 (a few seconds of stereo PCM, e.g. ~24 MiB), with a comment
  that it is a *rebuffer window*, not a track.
- Keep `dummy = false` and `transport = "auto"`.

**Files.** `src/main.rs`.

**Test.** Unit: generate config into a temp dir; assert the defaults and that the server boots with
them.

**Acceptance.** A default install plays a normal‑length track by default at best quality.

---

# Part B — Termux / Android‑specific fixes

## A1. Link `libopus` (and keep `libpulse`) for the NDK build

**Problem.** The NDK build links `libpulse.so` with no `DT_RUNPATH`, so Termux must inject
`LD_LIBRARY_PATH=$PREFIX/lib` at runtime (already done in the runit `run` script for `libpulse`).
Adding the Opus codec (G1) adds a `libopus.so` dependency with the same problem.

**Change.**
- `build.rs`/linker config: when `target_os = "android"`, link `libopus` (and keep `libpulse`) so
  `DT_NEEDED` is correct; do **not** rely on `dlopen`.
- Install/run scripts: the existing `LD_LIBRARY_PATH=$PREFIX/lib` export already covers the whole
  dir; ensure `libopus.so` is present in `$PREFIX/lib` and list it as a dependency in
  `install.sh`/README.
- Verify with a clean boot: no `CANNOT LINK EXECUTABLE` / no `library "libopus.so" not found`.

**Files.** `build.rs`, `install.sh`, the runit `run` script, README dependency list.

**Test/Acceptance.** `readelf -d l337-audio-server | grep -E 'libopus|libpulse'` lists both; server
boots and plays Opus on device.

## A2. PulseAudio‑first backend on a guaranteed real sink

**Problem.** The server's Android backend tries PulseAudio and falls back to AAudio; AAudio is
unreliable from a console/runit process, and if PulseAudio has no real sink the output is silent
even when decode succeeds.

**Change.**
- Keep PulseAudio primary (`AndroidAudioBackend::start_stream` already prefers PA when the
  `pulseaudio` feature is on). Fall back to AAudio only when PA is genuinely unavailable, and log the
  chosen backend (today it logs `Using PulseAudio backend for Android`).
- Guarantee a real output device: ensure the PulseAudio sink module `module-sles-sink`
  (`OpenSL_ES_sink`) is loaded (Termux `pulseaudio` normally provides it); document/enforce in
  `install.sh`. Do not accept a `module-null-sink` default.
- Orchestration: on Termux the player is a client of an external server; have the install start
  `pulseaudio --start` before/alongside the server service, and keep the runit `run` script's
  `LD_LIBRARY_PATH` + `PULSE_*` env needed to reach the per‑user daemon.

**Files.** `src/platform/android.rs`, `install.sh`, runit `run` script, README.

**Test/Acceptance.** `pactl list short sinks` shows `OpenSL_ES_sink`; `pactl list sink-inputs` shows
an **uncorked** sink‑input from `l337-audio-server` during playback with sink `State: RUNNING`.

## A3. AAudio fallback correctness (or make it explicitly opt‑in)

**Problem.** AAudio from a non‑Activity console process is unreliable on this device; when used, the
format/rate wiring must be exact or it is silent.

**Change.** Keep AAudio strictly as a fallback. When it is used, ensure format `float32`, shared
mode, low‑latency, adequate buffer capacity, and log the actual device rate (the code already warns
on mismatch). If it can't be made reliable, keep it disabled‑by‑default behind a config flag and
document PulseAudio as the supported Android backend.

**Files.** `src/platform/android.rs`, `src/main.rs` (optional flag), README.

**Acceptance.** On Termux the PA path is always taken; AAudio path (when exercised) is correct or
cleanly unavailable.

## A4. Deployed service reads config from the **service dir**, not the repo root

**Problem.** `sv restart` runs the server with `CWD = $PREFIX/var/service/l337-audio-server`, so the
server loads `server.ini` from **there**, not from the repo checkout. Empirically, editing the
repo‑root `server.ini` had no effect; only the service‑dir `server.ini` (which had
`buffer_max_bytes = 8388608`) governed the running server, and a regenerated service config silently
reverts to the bad defaults.

**Change.**
- `install.sh`/`dev-install.sh`: when enabling the runit service, write the canonical `server.ini`
  (G6 defaults: `default_target_kbps`, correct `buffer_max_bytes`) into
  `$PREFIX/var/service/l337-audio-server/server.ini`, and restart the service.
- Make the `run` script set env so config isn't CWD‑dependent: `L337__SERVER__DEFAULT_TARGET_KBPS`
  and `L337__SERVER__BUFFER_MAX_BYTES`.
- Document the CWD gotcha so it isn't rediscovered.

**Files.** `install.sh`, `dev-install.sh`, runit `run` script, README.

**Test/Acceptance.** After install/restart, `/player/status` behaves per the new defaults; no 8 MiB
regression on reinstall.

## A5. PulseAudio recovery crash specifics

**Problem.** G5's recovery design is global; on Termux the concrete crash is in
`src/platform/pulseaudio.rs` (`writer_loop` → backend error → `teardown` → `pa_stream_disconnect`
assertion). PulseAudio is compiled only for `target_os = "android", feature = "pulseaudio"`, so it
lives here as the platform‑concrete part of G5.

**Change.** In `PulseAudioHandle::teardown`/recovery: take the mainloop lock, ensure the writer
thread has fully stopped (join/fence via the `cancel` flag + `thread::JoinHandle`) **before**
`pa_stream_disconnect`/`pa_stream_unref`, and make teardown idempotent (guard against double
disconnect when both `stop()`/`Drop` and `try_recover` fire). This is the exact assertion failing
today.

**Files.** `src/platform/pulseaudio.rs`.

**Test/Acceptance.** Repeatedly force backend errors; the process never aborts; a fresh stream is
created and playback resumes (or cleanly reports stopped).

## A6. Socket transport + runtime env on Termux

**Change.** Keep `--transport=socket` binding to `$TMPDIR/l337-audio-server/runtime/l337.sock`
(`runtime_dir()` for android already does this). Ensure the runit `run` script and any app‑spawned
server both set: `LD_LIBRARY_PATH=$PREFIX/lib` (covers `libpulse` + new `libopus`), and inherit the
Termux env so `yt-dlp` (needed for G2/G3) is on `PATH`. Confirm socket mode keeps `/setup` enabled
for local token discovery as today.

**Files.** runit `run` script, `src/platform/android.rs`, README.

**Acceptance.** Client connects over the Unix socket; token discovery works; `yt-dlp` resolves on the
server side.

---

# Part C — Cross‑repo compatibility (player client, already changed)

The player (`l337-player`, `src/client/core/player.py`) now, for a YouTube **play**, fetches a
complete server‑decodable AAC/m4a with yt‑dlp and pushes via `POST /player/play/stream`, with a longer
timeout, and falls back to the old behaviour otherwise. That stopgap exists precisely because of the
missing Opus + broken fMP4 streaming (G1/G3), and it stays fully compatible with this plan:

- Once G1 (Opus) and G3 (container‑aware streaming) land, the client can send the YouTube **page
  URL** plus `target_kbps` (or a `quality` label) and let the server honor the ceiling; the stopgap
  becomes a fallback only for servers advertising an older capability set.
- Default (no `target_kbps`, no `quality`) == `best` == the client's current "highest decodable"
  choice, so nothing regresses when the client omits the fields.
- Keep `_server_can_resolve` keyed on `capabilities.yt_dlp`; adopting `target_kbps` is purely
  additive on the client.
- Front‑ends that prefer labels should render the canonical ladder from G2 (local UI copy of the
  table) rather than inventing their own numbers.

# Testing strategy (device‑validated)

- **Isolated instance harness:** run a second server with a private `TMPDIR` (own socket +
  single‑instance lock), `LD_LIBRARY_PATH=$PREFIX/lib`, `RUST_LOG=<crate>=debug`, optional
  `L337__SERVER__…` overrides — leaves the supervised server untouched. Used to reproduce every
  defect above without disrupting the running service.
- **Audio‑out verification without a microphone:** `pactl list sinks` → `State: RUNNING`;
  `pactl list sink-inputs` → an uncorked `l337-audio-server` input on `OpenSL_ES_sink`; sample
  `Buffer Latency` decreasing in real time == audio is actually reaching the speaker.
- **Unit tests** (`cargo test`) for label/selector mapping (G2), codec decode of Opus/WebM + AAC/m4a
  fixtures (G1), buffer window/drain + pacing math (G4), and idempotent teardown (G5).
- **Integration** on device: one id per source class per tier (YouTube Opus/WebM, AAC/fMP4,
  Vorbis/Ogg, a local FLAC) asserting `actual_kbps <= target`, `position_sec` advancing ~1×, no crash,
  and (per A2) an active uncorked sink input.

# Risks / non‑goals

- **`abr` is approximate:** yt‑dlp's average‑bitrate bounds are estimates; always keep the trailing
  `…/bestaudio` fallback so selection never fails, and report `actual_kbps` so callers see reality.
- **Ceiling tiers are advisory:** a source may have no variant at/below the ceiling. Transcode‑down
  (needs ffmpeg) closes most gaps; when it can't, clamp to the lowest available and report, never
  fail playback.
- **Transcode‑down needs ffmpeg**; where absent, degradation is nearest‑rendition + reported
  `actual_kbps`.
- **`lossless` is only bit‑exact for bit‑exact sources:** for lossy‑only content it degrades to
  `best` and says so (no false pretense of lossless).
- **Labels are the stable contract; kbps boundaries can be retuned per era.** Clients should render
  the labels the server advertises rather than hard‑coding bitrates.
- **Non‑goal:** this is a *target ceiling*, not adaptive bitrate. It does not measure throughput and
  re‑negotiate on the fly (a future enhancement could, using the same `target_kbps` primitive).
- **Non‑goal:** not implementing decoding beyond Symphonia's codecs. "All formats the server will be
  handed" means AAC/Opus/Vorbis/MP3/FLAC/ALAC + PCM/WAV in MP4/WebM/Ogg/Riff, with enough headroom for
  the lossless path used at `best`/`lossless`.
- **Client stopgap remains safe** until the server ships G1+G3; both sides keep the fallback path.

---

# Plan status — 2026-10-31 (installer round)

Updated after the installer hardening round described in `bugs.md` / `bugs2.md`. Engine-level goals
(G1–G6) are unchanged from their entries above; this section records what is now done, and what
remains, for the plan items this round touched.

## Closed

| Plan item | Status |
|---|---|
| **A6 — socket transport + runtime env** | Done and now *verified at install time*. The installer checks the service is supervised **and** that the server actually bound its Unix socket (`$TMPDIR/l337-audio-server/runtime/l337.sock`) before declaring success. Previously it probed TCP `:1337`, which the `--transport=socket` build never opens. |
| **A2 — PulseAudio-first backend / start the daemon** | The installer now starts the PulseAudio daemon when it is down (auto-fix, not an instruction), verifies it, and the verification distinguishes a *loader* failure (missing `libopus.so`, where `dummy = true` cannot help because `main()` never runs) from an audio-backend failure. |
| **A4 — deployed service reads config from the service dir** | Partially closed: the installer preserves and restores the service dir's `server.ini`, and the run script exports `LD_LIBRARY_PATH`. The CWD gotcha is documented. The remaining half is the unresolved `/etc/l337-audio-server/config.toml` question below. |
| **Part C cross-repo stopgap** | Unaffected: the client still sends the YouTube page URL + `target_kbps` when the server advertises `yt_dlp`; the server now honours the ceiling with the release it actually installs. |

## Remaining gaps

- **`install-systemd.sh` writes `/etc/l337-audio-server/config.toml`**, which `load_settings()` never
  reads (it reads `…/config` and `…/server.ini`), so the systemd installer's host/port/token are
  silently ignored. **Needs a decision on which file is authoritative** — carried from `bugs.md` §7.
- **The systemd/launchd enable-start-verify paths are not device-verified** — no Linux or macOS host
  here. They poll (`wait_until_ok`, 15 s) instead of a fixed sleep, which removes the false
  rollback-on-slow-start class of bug, but the exact interaction with a real PipeWire/ALSA sink is
  unconfirmed.
- **G1/G4/G5 device-only checks** remain as in `bugs.md`: `position_sec` advancing on a real sink,
  RSS reclaim, forced backend-error recovery, and the real YouTube ceiling. The engine fixes are
  still not in a published release, so a release-binary run cannot yet demonstrate them.
- **BUG-4** (best = highest available rendering) stays open — no resolvable YouTube target on this
  device.

## Addendum — 2026-10-31 installer verification round

- **A1 (linkage) is now verified at install time, not just at build time**: the
  installer checks the downloaded binary's published checksums (md5, and crc32 via
  the universally available `cksum`) plus its byte size before deploying it, so a
  truncated or corrupted transfer cannot reach `$PREFIX/bin`. Platform installers
  still verify `DT_NEEDED` libraries via `readelf` as before.
- Remaining as stated above: the systemd/launchd enable-start-verify paths are not
  device-verified here, `install-systemd.sh`'s `config.toml` question is open, and
  the G1/G4/G5 device-only playback checks still need a host with a real sink.
