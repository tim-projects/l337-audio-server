# bugs2 — Remaining playback defects after the Opus/pacing pass

**Component:** `l337-audio-server`
**Binary:** `~/.local/share/l337-player/bin/l337-audio-server` (mtime `2026-10-29 02:42`, now links `libopus.so`)
**Device:** Termux/Android, runit‑managed (`sv status` → `run: (pid 22274)`), transport `socket`
(`unix://$TMPDIR/l337-audio-server/runtime/l337.sock`), PulseAudio → `module-sles-sink` / `OpenSL_ES_sink`
**Related:** `plan-compatabikity.md` (G1 done; G2 partially present; G4/G5 still open)
**Reproduced with:** a dependency‑free stdlib HTTP client against the live Unix socket (the device Python
currently has no `httpx`/`pytest`/`textual`), so requests below are exactly what the player app sends.

---

## What already works (scope of "remaining" bugs)

- Opus is compiled in (`readelf -d` → `NEEDED libopus.so`). `/health` → `{"capabilities":{"yt_dlp":true},"status":"ok"}`.
- `POST /player/play` with the **YouTube page URL** (the natural path) resolves, decodes and starts:
  `{"ok": true, "state": "playing"}`, `duration_sec = 231`, `audio_available = true`.
- Audio reaches the speaker: `OpenSL_ES_sink` `State: RUNNING`, an **uncorked** sink‑input
  `application.name = "l337-audio-server"` (`float32le 2ch 48000Hz`), and `Buffer Latency` decreases
  ~2 s per 2 s wall‑clock → PA is rendering to OpenSL at 1×.
- The G2 API surface from the plan exists: `PlayerStatus` reports `target_kbps`, `actual_kbps`, `codec`.
- `/player/stop` → `state: stopped`.

So decoding (Opus/AAC) and the output path are fixed. The defects below are all in the
**streaming → writer → sink** lifecycle and end‑of‑track handling in `src/player/engine.rs`
(`start_streaming_playback`, `streaming_decode_sync`, `get_status`) and the platform writer loops
(notably `src/platform/pulseaudio.rs::writer_loop` and `src/platform/common.rs::AudioBuffer`).

---

## BUG‑1 — Playback timeline resets/loops every ~10 s while streaming

**Symptom.** During streaming playback `position_sec` climbs 0..~9 s then jumps back to 0 and repeats,
roughly a 10 s cycle. The track never plays "straight through"; the app's progress jumps backward and
`duration_sec` stays fixed but is never marched toward.

**Observed timeline** (`GET /player/status` once per second after `POST /player/play`):

```
t= 0 pos=0   t= 1 pos=1  t= 2 pos=2  t= 3 pos=2  t= 4 pos=4  t= 5 pos=4  t= 6 pos=5
t= 7 pos=6   t= 8 pos=7  t= 9 pos=8  t=10 pos=9  t=11 pos=0  t=12 pos=0  t=13 pos=1
...
t=22 pos=9   t=23 pos=0  t=24 pos=1  ...   (resets at ~t=11, ~t=23, …)
```

**Why it matters.** The player reads position for its progress bar and to know when a track ends. A
resetting playhead shows the wrong track position and interacts badly with BUG‑3 (EOS).

**Root‑cause hypothesis (to confirm in `engine.rs`).** `get_status` derives position from
`AudioBuffer::read_pos` (`read_pos → file frames → seconds`). A drop to 0 means either `read_pos` is
reset, or `streaming_decode_sync`/`start_streaming_playback` is re‑entered and rebuilds/clears the
buffer. The ~10 s period suggests the streaming download/decode thread finishes the initial
prebuf, drains it, and then **re‑initialises the decode from offset 0 and re‑appends** instead of
continuing the Opus stream — i.e. the streaming reader is a loop that reopens/restarts rather than a
single continuous pass. It is very likely the same root cause as BUG‑2 (a streaming path that re‑decodes
and re‑appends).

**Fix.** Make the streaming decode a single continuous pass over the downloaded file: keep one decode
cursor (file offset) that advances monotonically, never reset `read_pos` to 0 mid‑track, and never
re‑invoke `start_streaming_playback` for a track already loading/playing. Position must be monotonic
from 0 → `duration`.
**Verify.** Poll `/player/status` for a full track; `position_sec` increments ~1/s with no backward jump
and reaches `duration_sec` once.

---

## BUG‑2 — Unbounded streaming to the sink (≈9975 s backlog; memory grows)

**Symptom.** PulseAudio's `Buffer Latency` for the server's sink‑input grows to a huge backlog and only
drains at real time, so the speaker is fed from a massive queue rather than a small window. The track is
231 s, yet the sink held ~**9975 s** of audio (~43× the track) — i.e. the stream was decoded and
appended ~43 times to the same sink‑input without any bound or reclamation.

**Observed** (`pactl list sink-inputs` during playback):

```
Sink Input #5  application.name = "l337-audio-server"
Sample Specification: float32le 2ch 48000Hz   Corked: no
Buffer Latency: 9975562100 usec
              → 9973562417 usec
              → 9971562757 usec
              → 9969438109 usec   (≈ 2,000,000 µs / 2 s wall ⇒ draining at 1×)
```

Disk‑pool utilisation during playback was ~17.7 MB (the full track downloaded first — fine), so the
problem is the **buffer beyond decode**, not the download.

**Why it matters.** RSS on the server (and PulseAudio's server‑side buffer) grows without limit over a
session; a long session will OOM or be killed, and it interacts with the old `buffer cap exceeded`
assertion once the AudioBuffer cap is hit again.

**Root‑cause hypothesis.** The writer loop (`pulseaudio.rs::writer_loop`, and the equivalents in each
`src/platform/*` backend) writes as fast as it can with no real‑time pacing (only sleeps when there's
*no* data), and `AudioBuffer` is never drained/reclaimed as `read_pos` advances — so decoded PCM
accumulates and is written faster than it can be played. Combined with the BUG‑1 re‑append loop, `n`
re‑decodes are appended to one open stream. This is exactly plan item **G4** (bounded, real‑time‑paced
window), which is not yet in effect.

**Fix (from plan G4).**
- Pace the writer to the device clock: after writing `n` frames, wait ~`n / sample_rate` (or hold a
  next‑deadline), so it consumes at 1×.
- Enforce the cap on the **unconsumed window**: `(pcm.len() - read_pos) > max_bytes/4`, and reclaim by
  `pcm.drain(..read_pos); read_pos = 0` at a checkpoint (under the existing `Mutex`).
- Treat `buffer_max_bytes` as a rebuffer **window** (a few seconds), not a whole track; once this is in,
  the 8 MiB default stops being a cliff for full‑length songs.
- Do not let a re‑append path (BUG‑1) reopen/append on a stream that is already playing.

**Verify.** `pactl list sink-inputs`: `Buffer Latency` stays bounded to a few seconds (≈ the rebuffer
window) and drains ~1 s per 1 s wall; server RSS is flat across a full track; play a full track with no
`buffer cap exceeded` and no abort.

---

## BUG‑3 — No end‑of‑track: state stays `playing` after playback finishes

**Symptom.** Neither the position nor the state ever signals completion. Historically the engine left
`state: playing` with `position_sec` frozen once it ran out of decoded audio; with BUG‑1 the timeline
rewinds instead of reaching the end, so there's still no completion event.

**Why it matters.** The player auto‑advances to the next playlist row only when the server's state
transitions out of `playing`/`paused` (it watches for `playing → not playing`). If the engine never
emits `stopped` at track end, the UI never advances to the next track.

**Fix.** When the writer reaches `duration` (decoded fully and drained), transition
`state = Stopped` (and reset `position_sec`, leaving `audio_available` as appropriate). If a
"gap/auto‑continue" mode is desired, expose it explicitly rather than staying `playing` forever.
**Verify.** Play a short track to completion; observe `state` go `playing → stopped` right when
`position_sec` reaches `duration_sec`, and the client advances to the next track.

---

## BUG‑4 — "best" quality selects a low‑bandwidth renditions instead of the highest available (confirm intent)

**Symptom.** At `target_kbps = 0` (best), the server selected and reported `codec: aac`,
`actual_kbps: 129` for this YouTube track. YouTube offers a higher‑bitrate Opus (`bestaudio`, itag 251,
~130–160 k Opus) and the server can now decode Opus, yet the "best" selection lands on the ~129 k AAC
(m4a) rendition.

**Why it matters.** "best/lossless" should mean the highest available representation, and this is the
default. Under‑selecting silently reduces quality for everyone by default.

**Fix.** Align the best/`target_kbps=0` selector with plan G2's "best = highest available" mapping and
confirm the intended preference order once Opus decode is available. Make the mapping observable (the
status already reports `actual_kbps`/`codec`, which is good — use it in tests).
**Verify.** For a video that has both AAC and Opus, `best` reports the higher‑bitrate variant and the
matching `codec`, and lower tiers correctly clamp below it.

---

## NOTES — Integration/schema (cross‑repo, low priority)

- **Status shape vs client progress bar.** The player reads a human `status['time']` of the form
  `"m:ss"/"m:ss"`; the server returns `position_sec`/`duration_sec` ints, so the client progress bar
  stays empty. Pick one on the wire: either add `time` (`"#{pos//60:02}:{pos%60:02}/#{dur//60:02}:{dur%60:02}"`)
  to `PlayerStatus`, or (player side) read `position_sec`/`duration_sec`. Bugs 1/3 must land first or the
  bar will still be wrong.
- **Capability advertisement.** `/health` still returns only `{"yt_dlp": true}`. Since Opus (and the
  bandwidth API) are now supported, advertise them (e.g. `capabilities: { yt_dlp: true, codecs: [...],
  quality: true }`) so the player can switch off its stopgaps and expose quality presets.
- **Whole‑track pre‑download.** ~17.7 MB disk‑pool use and a pre‑play buffering delay show the file is
  downloaded before streaming (plan G3: fMP4 → full download, Opus/WebM → incremental). For Opus/WebM,
  streaming incrementally would cut time‑to‑first‑sound; not a correctness bug.

---

## Evidence appendix

- `readelf -d …/l337-audio-server | grep NEEDED` → `libaaudio.so`, `libpulse.so`, **`libopus.so`**.
- `GET /health` → `{"capabilities":{"yt_dlp":true},"status":"ok"}`.
- `POST /player/play {"track_id":"4d15b27f…","stream_url":"https://www.youtube.com/watch?v=bZiCchjRW_s","title":"I Need Your Lovin'","artist":"N-R-G - Topic","duration":231}`
  → `{"ok": true, "state": "playing"}`.
- Representative `GET /player/status`:
  `{'state':'playing','volume':1.0,'speed':1.0,'pitch':1.0,
    'current_track':{...'duration':231,'target_kbps':None,'quality':None},
    'disk_pool_utilization_bytes':17720587,'next_cached':False,'prev_cached':False,
    'position_sec':1,'duration_sec':231,'audio_available':True,'target_kbps':0,'actual_kbps':129,'codec':'aac'}`
- Position monotonic‑then‑reset trace: see BUG‑1.
- `pactl list sink-inputs` Buffer Latency trace: see BUG‑2.

## Priority

1. **BUG‑2** (fixes OOM risk + is half of BUG‑1's re‑append; plan G4).
2. **BUG‑1** (continuous single‑pass streaming; shares the G4 root).
3. **BUG‑3** (EOS → `stopped`, so the app advances).
4. **BUG‑4** (best = highest available).
5. Integration NOTES (`time` field, capability advertisement) opportunistically.

---

# Update — 2026‑10‑30 device session (Termux/aarch64, live PulseAudio backend)

Ran the released `2026.10.29-5` binary against the live OpenSL ES sink, then built and ran
the fixed tree on‑device (`cargo build --features pulseaudio,opus`, host triple
`aarch64-linux-android`, dev profile). Status per bug:

- **BUG‑1 — ROOT‑CAUSED AND FIXED (verified on‑device).** Not a streaming re‑decode as
  hypothesised: the playback timeline rewound every **~10.9 s**, exactly the writer's
  `read_pos >= 1<<20` checkpoint. `AudioBuffer::reclaim_consumed()` drains the consumed
  prefix and resets `read_pos = 0`, while both position computations
  (`calculate_position`, `get_status`) derived the playhead from `read_pos` alone — so
  every reclaim jumped back to the seek base. Fix: `AudioBuffer::consumed_total` counts
  reclaimed samples; position = `consumed_total + read_pos`; reset at every buffer refill.
  Live trace after the fix: `0,1,2,…,8,10,11,…,29` then EOS — no sawtooth. Unit test
  `test_position_survives_writer_reclaim` guards it.
- **BUG‑2 — fixed, verified bounded.** One 30 s track: PA `Buffer Latency` ~38 s flat
  (≈ 1× track + the SL ES queue), not the 9975 s backlog; server‑side window drains at
  ~1× (position advanced 1 s per 1 s). The residual is this device's OpenSL sink not
  rendering in real time, not a server defect.
- **BUG‑3 — FIXED (verified on‑device).** `get_status` now ends playback as
  `state = stopped` (position reset) once the decode task is finished and the buffer is
  fully drained — the live trace shows `playing → stopped` at track end, so the app's
  auto‑advance transition fires. A drained buffer with decode still running stays
  `playing` (rebuffer, not EOS).
- **BUG‑4 — still open**, unverifiable here (no resolvable YouTube target on this device).

## Other defects found and fixed in this session

1. **Installer gap (the reported 404).** `install-android-termux.sh` picked the newest
   prerelease and 404'd when a partial release lacked our asset (`2026.10.29-6/-7` ship
   only the x86_64 Linux assets). It now walks releases newest‑first and picks the newest
   whose **release-info.json confirms the wanted asset**, using the API's own
   `browser_download_url`. Same walk added to `install-macos.sh`; `install.ps1` mirrors it
   (not executed here — no pwsh on device). Verified end‑to‑end on this device: walks
   `-7 → -5`, installs and registers the service, `/health` + `/version` ok.
2. **`cargo test` was never executed** (Linux host ran `cargo check` only). First real run
   exposed: pitch≠1 tracks playing `1/pitch` too long (`tempo = speed / pitch` assumed a
   length‑rescaling pitch shifter; it is length‑preserving — fixed in both the file and
   streaming paths), the fake `yt-dlp` test fixture using a `#!/bin/bash` shebang that
   cannot exec on Termux (`/bin → /system/bin`), so execvp silently fell through to the
   real yt‑dlp — changed to `#!/bin/sh`; the endpoints tests guessing the server binary
   path — now `env!("CARGO_BIN_EXE_…)`; and 22 parallel test servers stealing each other's
   single‑instance lock/token — now per‑spawn `TMPDIR`/`XDG_RUNTIME_DIR`/`STATE_DIRECTORY`
   isolation. Suite: **45/45 unit + 22/22 endpoints green** (parallel).
3. **Dev‑profile hazard (not a release blocker):** `axum-server` + tokio 1.52
   `debug_assert!` panics when registering the blocking Unix socket in socket mode —
   release builds (debug‑assertions off) are unaffected (why `-5` runs fine). On‑device
   builds need `RUSTFLAGS="-C debug-assertions=off"`.

## Device environment notes

- `/tmp` is **not writable** by the Termux app user (`drwxrwx--x shell:shell`); `TMPDIR` is
  `$PREFIX/tmp`. Build with `CARGO_HOME=$HOME/.cargo CARGO_TARGET_DIR=$TMPDIR/…`.
- Disk is tight (~1.8 GB free): `cargo build --features pulseaudio,opus` succeeded
  (~5 min, 1.2 GB target dir), so on‑device verification of future engine fixes is viable
  without CI.
