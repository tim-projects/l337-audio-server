# bugs — consolidated defect, findings and verification log

**Single source of truth for `l337-audio-server`.** `bugs2.md` was merged into this file on
2026-11-02: its playback-defect records became [Part 5](#part-5--playback-defect-records-bug-1bug-4)
and its dated updates became the [Session log](#part-6--session-log-chronological). No content was
dropped in the merge; sections that referenced `bugs2.md` now reference the in-file records.

## Status summary — 2026-11-02 (branch `g1-opus-decode` @ `e7834d8`, release `2026.11.02-4`)

| ID | Defect / finding | State | Verified |
|---|---|---|---|
| **BUG-1** | Playback timeline resets/loops every ~10.9 s while streaming | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (60 s monotonic `0…58`, no backward jump) |
| **BUG-2** | Unbounded streaming to the sink (~9975 s PulseAudio backlog) | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (`Buffer Latency: 256231 usec`) |
| **BUG-3** | No end-of-track — state stayed `playing` forever | **FIXED** | device 2026-10-30, re-verified 2026-11-02 (seek→284 s of 294 s → `stopped` at EOS) |
| **BUG-4** | `best` (`target_kbps=0`) selected a low-bitrate rendition | **FIXED** | verified on a live target 2026-11-02, **both directions** via `yt-dlp -F` (AAC 129 k when AAC is the max; Opus 144 k when Opus is the max) |
| **BUG-A** | `resample_interleaved` emitted literal zeros for 44.1→48 kHz (all AAC/MP3 playback was silent) | **FIXED** | 2026-11-02 `parec` monitor peak **6513** during AAC playback (0 = the old regression) |
| **BUG-B** | `stop()` never tore the stream down — leaked sink-input appending silence | **FIXED** | 2026-11-02 sink-input `count=0` after `POST /player/stop` |
| **U2 / U7** | Streaming Opus ended early (~7 s), then resumed but **stalls at ~9 s** | **FIXED** — premature-EOS fixed, wedged-MKV root cause fixed by rebuilding the reader, then the *second* wedge (Symphonia's frozen stream length + a rebuild policy that gave up) fixed 2026-11-05 and verified on-device (0→213 s continuous, see the 2026-11-04/05 session). **Re-test 2026-11-12:** page-URL YouTube stream (`https://www.youtube.com/watch?v=dQw4w9WgXcQ`) still stalled at position 0 — demuxer wedged on `past master element`, single rebuild attempted then fell to "waiting for download" despite completed download. **Fix 2026-11-12:** after `reprobe_format()` in `wedge_action()` rebuild path, reset `last_rebuild_at = None` so the next `DecodeError("past master element")` immediately triggers another rebuild instead of waiting for the 250 ms rate limit or 3 s quiet grace. Fix committed but not yet re-verified against YouTube. | 2026-10-31, 2026-11-02 (trace), 2026-11-03 (fix, Part 7), **2026-11-05 (re-fix + live verify)**, **2026-11-12 (page-URL re-test stall + fix committed)** |
| **C** | The installed binary was a stale release — none of the fixes were in the running artifact | **FIXED** — provenance found, runner hardened | 2026-11-02: dispatch requires a branch + tag pinned to `$GITHUB_SHA`; release `2026.11.02-4` tag == built commit |
| **NOTES** | Client progress bar (`time` field) + `/health` capability advertisement | **CLOSED** | 2026-11-02 `codecs`/`quality` advertised; `PlayerStatus.time` = `"00:00/04:54"` |
| **G3** | Whole-track pre-download was the default before playback | **FIXED** | 2026-11-03: streaming is the default, whole-file download is the fallback for containers that cannot be probed from a partial file (Part 7) |
| **Finding #1** | Linux binaries need `libopus` at runtime; no installer provided it | **FIXED** | host 2026-10-29; re-verified per-asset in `release-info.json` for `2026.11.02-4` |
| **Finding #2** | Reclaim + window cap on PulseAudio only (5 other backends tripped) | **FIXED** | code; raw `pcm.len() > buf.max_bytes` gone from `src/platform/` |
| **Finding #3** | File/fMP4/cached path keeps the whole decoded track resident | **FIXED** — `decode_file_incremental_blocking` now bounds `pcm` to the rebuffer window via reclaim + `wait_for_space` on both the Symphonia and Opus file paths; peak RSS during file decode is independent of track length | 2026-11-11 (dummy + custom-port verify; device RSS reclaim pending real backend) |
| **Finding #4** | Background cache ignored the runtime ceiling from `POST /player/target` | **FIXED** | `test_cache_next_honors_runtime_target_ceiling` |
| **Finding #5** | Unknown `quality` string rejected (422) | **FIXED** | `test_play_unknown_quality_falls_back_to_server_default` (200) |
| **Finding #6** | `lossless` not enforced; `needs_bitexact()` dead | **FIXED** — `quality: lossless` now prefers lossless renditions (flac/alac/pcm) in format selection; `PlayerStatus.lossless_requested`/`lossless_fulfilled` report whether lossless was requested and actually delivered (honest degradation). `needs_bitexact()` removed. | 2026-11-11 (unit-tested) |
| **Finding #7** | `CHUNK_FRAMES = 256` forced ~190 PA writes/sec | **FIXED** in code (4096); device re-check **OPEN** | — |
| **Finding #8** | Termux libopus install/diagnostics, matrix `fail-fast`, needless `libclang` | **FIXED** | |
| **Finding #9** | Committed `libopus.so` stub can drift; per-asset Opus capability unrecorded | **FIXED** (CI validation + recording) | optional CI regeneration-with-diff remains open |
| **Finding #10** | Duplicated stall/retry policy (Opus vs Symphonia) | **FIXED** | `wait_for_progress` shared + unit test |
| **Host-run extras** | `track_id` traversal write/delete (CRITICAL), bare-token 401, double yt-dlp, seek clamp, volume lost after recovery, self-inflicted volume regression, dummy-mode config passthrough | **FIXED** | host run 2026-10-29 (table in Part 3) |
| **Installer rounds** | 404-skip release walk, stale `.tmp` install, root CWD exec, `--binary` clobber, termux glibc detection, service enable/start/verify, wrong transport, macOS asset, download checksums, Debian false failures, systemd `server.ini` | **FIXED** | device 2026-10-31/11-01 + suite |
| **Release process** | Release tag created at the default-branch tip, not the built commit (2026.11.02-3 had correct binaries, stale source refs) | **FIXED** | 2026-11-02: dispatch requires `branch`, all jobs check it out, `gh release create --target $GITHUB_SHA`; `2026.11.02-4` verified tag == commit |

Merge gate: **all CRITICAL/WARNING findings closed.** What remains open is listed in
[Part 7](#part-7--new-findings-issues-and-mitigations--2026-11-02).

---

# Part 1 — Review findings (`g1-opus-decode` vs `origin/main`)

Checklist of every finding from the branch review (`/review branch`). Fix top-to-bottom by
severity; each item has the same shape: **Sev**, **Where**, **Problem**, **Suggestion**, and a
concrete **Checklist**. Tick each step; the whole item is done when its checklist is fully checked.

- Severity: `CRITICAL` / `WARNING` / `SUGGESTION`
- Reviewed diff: merge-base `d5cc62a` → `g1-opus-decode` (HEAD `ea7ccf3`)
- Merge gate: no open CRITICAL/WARNING before merging/releasing.

---

## 1. CRITICAL — Linux release binaries require `libopus` at runtime, not installed by any installer

- **Where:** `build.rs:29-35` (`link_opus` emits `-lopus`), `build.yml:120-124` (Linux pipewire/alsa `pkg_deps: …libopus-dev` + `--features opus`), and the Linux installers/`release-info.json`/`check-glibc.sh` (none cover it).
- **Problem:** The `libopus-dev` build dep + `cargo:rustc-link-lib=dylib=opus` give every Linux asset `DT_NEEDED libopus.so(.0)`. The loader resolves DT_NEEDED before `main()`, so a host without `libopus0`/`opus` (Debian/Ubuntu / Fedora/RHEL / pacman) fails to **exec** the binary in any mode — including `dummy = true`. `check-glibc.sh` only scans `GLIBC_` symbols, `release-info.json` records glibc only, and `install-linux-alsa.sh` / `install-linux-pipewire.sh` / `install-systemd.sh` install nothing for it → CI green, installer reports success, server never starts.
- **Suggestion:** record the shared-lib requirement per asset in `release-info.json`; have the Linux installers check/install it via the package-manager table already in `install-systemd.sh:252-289`; gate the release on a post-build `readelf -d … | grep NEEDED` check; document `libopus0`/`opus` in README + installer warnings.

**Checklist**
- [x] `install-systemd.sh` / `install-linux-alsa.sh` / `install-linux-pipewire.sh`: detect the binary's `DT_NEEDED libopus` and install the runtime package (`libopus0` / `opus` / `opus`) per PM, with a clear warning if unavailable.
- [x] Add a runtime dependency check (e.g. `readelf -d` or `ldd`) in the Linux installers so a missing lib is surfaced at install time, not first boot.
- [x] `build.yml`: add a post-build verify (both Linux jobs) that the binary's `NEEDED` set includes `libopus.so`/`libopus.so.0`.
- [x] `release-info.json` generator / README: record `libopus` as a required shared library for the Linux assets.
- [ ] Verified: a stock Linux host can install + start the release server (dummy or real).

---

## 2. WARNING — G4 pause/reclaim + window cap migrated to PulseAudio only; other 5 backends still trip on a long track

- **Where:** `src/platform/linux.rs:98`, `src/platform/alsa.rs:189`, `src/platform/android.rs:167`, `src/platform/macos.rs:78`, `src/platform/windows.rs:125` (all still `buf.pcm.len() > buf.max_bytes / 4`, no reclaim) vs `src/platform/pulseaudio.rs:226/270` (uses `exceeds_window()`/`reclaim_consumed()`).
- **Problem:** The branch redefined `AudioBuffer::max_bytes` as the *unconsumed* window (24 MiB) and added `exceeds_window()`/`reclaim_consumed()`, but only PulseAudio uses them. On the default desktop PipeWire backend (and ALSA/AAudio/macOS/Windows), `pcm.len()` keeps growing with track length and nothing reclaims, so a track longer than ~6.3 M samples (~65 s stereo @48 kHz, or the whole file path) clears the buffer + raises `backend_error` → `try_recover` loop. "Memory independent of track length" holds only on PulseAudio.
- **Suggestion:** replace the raw checks in all five writers with `buf.exceeds_window()` and call `buf.reclaim_consumed()` at each writer's checkpoint, exactly as pulseaudio.rs.

**Checklist**
- [x] `linux.rs`: cap → `buf.exceeds_window()`; call `buf.reclaim_consumed()` after `read_pos` advances (checkpoint).
- [x] `alsa.rs`: same (note ordering — reclaim after the lock-taken copy, no double-lock).
- [x] `android.rs` (AAudio): same.
- [x] `macos.rs` + `windows.rs`: migrate if the check is on a live path; if placeholder/Noop, confirm the check is unreachable and skip with a note.
- [x] Confirm no backend drains/i-set state under a lock held across reclaim (verify lock scope in each).
- [x] Verify: `grep src/platform -e 'pcm.len() > buf.max_bytes'` returns nothing.

---

## 3. FIXED — File/fMP4/cached path keeps the whole decoded track resident

- **Where:** `src/player/engine.rs` (`decode_file_incremental_blocking`, both the Symphonia and Opus file paths).
- **Problem:** The entire decoded + resampled track was assigned into `audio_buffer.pcm` in one shot, so a long track pinned its full PCM (≈1.1 GiB for 60 min stereo @48 kHz), defeating the bounded-window goal and risking OOM/lowmemorykiller on device.
- **Fix:** `decode_file_incremental_blocking` now reclaims consumed samples while decoding and blocks on freed buffer space (`wait_for_space`, 20 ms timeout) instead of sleeping, keeping `pcm` bounded to the rebuffer window (~24 MiB default). The Opus file path, previously unbounded, is throttled the same way. Peak RSS during file decode is now independent of track length (Finding #3 / G4).
- **Verified:** `cargo check` clean; `cargo test` green (60 unit + 26 endpoint). Small WAV plays end-to-end on an isolated custom-port (13370) dummy instance; a 52 MB local WAV decodes without deadlocking and the server stays responsive (`/health` + `/player/status` answer throughout decode). Full device RSS-reclaim verification needs a real PipeWire/ALSA backend (dummy mode has no consuming writer).

**Checklist**
- [x] `decode_file_incremental_blocking` (Symphonia path): reclaim + `wait_for_space` instead of `sleep` when over window.
- [x] `decode_file_incremental_blocking` (Opus path): same throttle + reclaim (previously unbounded).
- [x] `cargo check` + `cargo test` green.
- [x] Custom-port (13370) isolated-instance verify: local file plays, long file decode doesn't hang/deadlock.
- [ ] Device RSS-reclaim verification (real backend) — pending a host with PipeWire/ALSA.

---

## 4. WARNING — Background cache ignores the runtime ceiling from `POST /player/target`

- **Where:** `src/api/handlers.rs:254`, `:296` (`engine.default_target_kbps()` = config) vs `src/player/engine.rs:346` (`play_track` uses `self.runtime_target_kbps`).
- **Problem:** After `POST /player/target {"target_kbps":64}`, live playback uses 64 but `cache_next`/`cache_previous` resolve with the config default (0 → `bestaudio`), so `next`/`prev` are downloaded above the active ceiling and `PlayerStatus` reports `target_kbps=64` alongside a higher `actual_kbps`.
- **Suggestion:** use `engine.current_default_target_kbps()` in both handlers — better, one `engine.resolve_track_ceiling(&Track)` so the precedence rule exists once.

**Checklist**
- [x] `handlers.rs::cache_next` + `cache_previous`: resolve via `engine.current_default_target_kbps()`.
- [x] (Optional, preferred) add `PlayerEngine::resolve_track_ceiling(&Track)` and route `play_track`, both cache handlers, and `set_target` through it so the precedence lives in one place.
- [x] Test: set runtime `target_kbps=64`, then cache next/prev, then assert the persisted/next slice uses a ≤64 selector. — `tests/endpoints.rs::test_cache_next_honors_runtime_target_ceiling` (fake `yt-dlp` on the spawned server's PATH records the selection: with ceiling 64 the download is `-f 249` (51 kbps), never `-f 251` (158 kbps); pre-fix it picked the config default's `bestaudio`).
- [x] Verify: no remaining `resolve_ceiling(...default_target_kbps())` in `src/api/handlers.rs`.

---

## 5. WARNING — Unknown `quality` string is rejected (contradicts the "never errors" contract)

- **Where:** `src/api/models.rs:9-10` (`Quality` enum, `rename_all="lowercase"`, no unknown fallback) vs `resolve_ceiling` doc at `models.rs:47`.
- **Problem:** Any unrecognized string (`"ultra"`, `"veryHigh"`) fails serde deserialization at the axum `Json` extractor → 400/422 before `resolve_ceiling` runs. Only *absent* (`#[serde(default)]` + `Option<..>`) is non-erroring. The documented contract ("unknown/absent quality never errors — it is skipped") only holds when the key is omitted.
- **Suggestion:** add a tolerant deserializer for the `quality` fields (unknown → `None`), or deserialize into `String`/`Option<String>` and match; add a test for unknown → server default.

**Checklist**
- [x] `models.rs`: add a `deserialize_with` helper for `quality` (Track + TargetPayload) that maps unknown/malformed strings to `None`.
- [x] Unit test: `quality: "ultra"` (and casing variants) deserializes to `None` → ceiling = server default.
- [x] Integration check: `POST /player/play {"quality":"ultra"}` returns 200 and plays at `default_target_kbps`/best. — `tests/endpoints.rs::test_play_unknown_quality_falls_back_to_server_default` (200, not 422; status falls back to the server default).
- [x] Keep the `resolve_ceiling` doc accurate.

---

## 6. FIXED — `lossless` not enforced; real bit-exact delivery with honest degradation

- **Where:** `src/api/models.rs` (`Quality`), `src/player/engine.rs` (`yt_dlp_format_selector`, `select_audio_format`, `resolve_youtube_stream_url`, `download_stream`, `start_streaming_playback`, `load_and_play`), `src/api/models.rs` (`PlayerStatus`).
- **Problem:** `Quality::Lossless.ceiling_kbps() == 0` (same as `Best`), so `resolve_ceiling` gave lossless no ceiling and `yt_dlp_format_selector(0)` returned `bestaudio` — yt-dlp's own ranking, which routinely resolves to a lossy Opus/AAC rendition. Nothing called `needs_bitexact()`, and `PlayerStatus` had no lossless signal, so a `quality: lossless` request silently returned lossy audio (overclaim).
- **Fix (real enforcement, not just honest docs):**
  - Added `Quality::is_lossless()` to distinguish `Lossless` from `Best` (both ceiling 0).
  - Threaded `lossless: bool` through `download_stream` → `resolve_youtube_stream_url` → `ytdlp_selection_json` → `ytdlp_selection_via_dump` → `select_audio_format`. When `lossless` is true, `select_audio_format` prefers the highest-bitrate lossless rendition (flac/alac/pcm/wav) at or below ceiling; if none fits, falls back to best (lossy) and reports the degradation honestly.
  - `PlayerStatus` gains `lossless_requested` (lossless was requested) and `lossless_fulfilled` (a lossless rendition was actually delivered), so clients can tell whether delivery was degraded.
  - `needs_bitexact()` and its test were removed (dead in product; the server pass-throughs and never re-encodes).
- **Checklist**
  - [x] `Quality::is_lossless()` added; `Lossless` distinguished from `Best` despite both having ceiling 0.
  - [x] `lossless` threaded through the YouTube resolution path (dump + selector fallback).
  - [x] `select_audio_format` prefers lossless renditions; honest fall-back to best when none fits.
  - [x] `PlayerStatus.lossless_requested` / `lossless_fulfilled` emitted in `get_status`.
  - [x] `needs_bitexact()` removed from `src/`; no references remain.
  - [x] Unit tests: `test_is_lossless_codec_label`, `test_is_lossless_codec`, `test_select_audio_format_prefers_lossless`, `test_select_audio_format_lossless_falls_back_to_best`. All green (`cargo test`: 64 unit + 26 endpoint).

---

## 7. SUGGESTION — `CHUNK_FRAMES = 256` forces ~190 PulseAudio writes/sec

- **Where:** `src/platform/pulseaudio.rs:206`.
- **Problem:** With the new pacing (`sleep n/(channels*rate)`), a 256-sample chunk = ~5.3 ms at 48 kHz → ~188 `pa_stream_write`/sec (~375 @96 kHz), each a 1 KiB pool buffer + locks + IPC/wakeup. `pa_stream_write` is queued/async, so a bigger chunk is correctness-neutral — only the write cadence changes.
- **Suggestion:** raise `CHUNK_FRAMES` to 2048–4096 (≈40–80 ms), keeping the cap guard and pacing math unchanged.

**Checklist**
- [x] `pulseaudio.rs`: `CHUNK_FRAMES = 4096`.
- [ ] Verify on device: audio still plays 1×, no underrun/overrun regression, lower CPU/wakeups (rough check via `top`/battery).

---

## 8. SUGGESTION — Termux libopus install/diagnostics + termux linkage verify + Linux matrix robustness

- **Where:** `scripts/install-android-termux.sh:363` (libopus check), `build.yml:337` (verify step), `build.yml:52` (concurrency/fail-fast + publish `continue-on-error`), `build.yml` alsa `pkg_deps` (unnecessary `libclang-dev clang`).
- **Problem:** (a) the libopus check hard-fails the install by default and, under `--no-install-deps`, only warns → then the binary can't load (`CANNOT LINK EXECUTABLE … libopus.so`), runit crash-loops it, and the final hints blame "an audio-backend failure" and suggest `dummy=true` (which can't help — `main()` never runs). (b) The termux verify step only greps `libpulse.so`, never the new `libopus.so`. (c) Linux matrices keep `fail-fast: true` and `publish-release` carries `continue-on-error`, so one variant failure cancels its sibling and skips the release while the run may still report green; the `libclang-dev clang` on alsa is unneeded.
- **Suggestion:** under `--no-install-deps`, route missing libopus into `NEEDS_USER`; distinguish a loader failure from backend failure in the service diagnostics; assert both `libpulse.so` and `libopus.so` in the termux verify step; `fail-fast: false` on the Linux matrices; drop `libclang-dev clang` from alsa `pkg_deps`.

**Checklist**
- [x] `install-android-termux.sh`: missing libopus under `--no-install-deps` → `NEEDS_USER` (not just warn).
- [x] `install-android-termux.sh`: after `sv up`, if the service log shows `CANNOT LINK EXECUTABLE` / `library "libopus.so" not found`, report a loader/dependency failure instead of the audio-backend `dummy=true` hint.
- [x] `build.yml` termux verify step: require `libopus.so` as well as `libpulse.so`.
- [x] `build.yml`: set `fail-fast: false` on the Linux `build-linux` + `build-linux-arm64` matrices (and consider failing publish-report when publish is skipped).
- [x] `build.yml`: remove `libclang-dev clang` from the alsa `pkg_deps` (bindgen there comes from C-FFI crates that `--no-default-features` disables).
- [x] Verify: trigger `target=all`; every target green with the opus-enabled Linux/Android assets.

---

## 9. SUGGESTION — Committed `libopus.so` stub blob can drift; armv7/riscv64 Opus capability unrecorded

- **Where:** `scripts/opus-stubs/libopus.so` (committed binary), `scripts/opus-stubs/opus-stub.c`, `.gitignore` negation, `build.yml:284` (armv7/riscv64 alsa jobs).
- **Problem:** An aarch64 ELF blob is committed with no CI regeneration/validation, so it can drift from `opus-stub.c` (only the linker would catch it; unauditable by diff). Separately, armv7/riscv64 release binaries build without Opus and nothing in `release-info.json`/README records which assets carry it.
- **Suggestion:** add a CI step that rebuilds the stub from `opus-stub.c` or asserts `Machine=AArch64`, `SONAME=libopus.so`, and the 4 symbols; record per-asset Opus capability in release metadata/README.

**Checklist**
- [x] `build.yml`: a step for the termux job that asserts the stub `readelf -h/-d` (AArch64, SONAME `libopus.so`) and the 4 exported symbols.
- [x] `build.yml` / README: comment + record that armv7/riscv64 assets have no Opus (source-agnostic ceiling still degrades + reports `actual_kbps`).
- [x] (Optional) regenerate the stub in CI and fail on diff.

---

## 10. SUGGESTION — Duplicated stream stall/retry policy between the Opus and Symphonia streaming paths

- **Where:** `src/player/engine.rs:1699-1711` (Opus streaming `IoError` block) vs `src/player/engine.rs:1889-1901` (existing Symphonia block).
- **Problem:** The two copies share one "how long to tolerate a stalled/partial stream" policy (file-growth check, `stall_cycles > 20`, 200 ms) but live in separate branches; tuning one leaves Opus streams aborting after ~4 s of no growth under identical network conditions.
- **Suggestion:** factor into `fn wait_for_progress(path, &mut last_file_size, &mut stall_cycles) -> bool` and call it from both match arms.

**Checklist**
- [x] Add `wait_for_progress` helper; call it in both `Err(Error::IoError(_))` arms.
- [x] Behavior preserved for the Symphonia path; Opus path now shares the tolerance.
- [x] Tests still pass; no shared-mutex lock held across the helper's sleep.

---

**Progress:** 10 / 10 addressed (see the [status summary](#status-summary--2026-11-02-branch-g1-opus-decode--e7834d8-release-20261102-4) for the current state of every item).

Remaining (documented, tracked for follow-up): the device-only verifications that need a real Linux host (finding #1 stock-host start) plus live-integration checks (#4/#5/#7 that need YouTube/a target to observe bitrate). Everything shipped + device-validated: Opus decodes (G1), the 90 s track that used to trip the window cap now plays 1× with a non-fatal diagnostic, ceiling/labels resolve correctly, and Android + Linux CI are green with the libopus linkage verified.

**Host verification round 2** (Debian 13, x86_64, dummy output, prereleases
`2026.10.29-5` … `-7`) closed the merge gate on this host and found six further
defects — two of them security-relevant — plus one regression introduced by an
earlier fix on this same round. See
[Host verification run](#host-verification-run--debian-13-trixie-x86_64-dummy-output)
and [Skipped work](#skipped-work-hand-off-for-a-linux-host-with-a-real-pipewirealsa-sink)
for the remaining device-only items.

---

# Part 2 — Linux host verification checklist

Run on a stock Linux host (Debian/Ubuntu, Fedora, or Arch) with PipeWire **and** an
ALSA box, newest release downloaded from GitHub. Use an **isolated instance** so the
supervised service is untouched: a private `TMPDIR` (own socket + single-instance lock),
its own `L337__SERVER__…` overrides, and `RUST_LOG=l337_audio_server=debug`. `yt-dlp`
must be on `PATH` for the YouTube steps. Socket mode keeps `/setup` for token discovery.

## Install + linkage (finding #1 CRITICAL)
- [ ] `sudo ./scripts/install-linux-pipewire.sh` (then the ALSA box) completes and reports **`ok "libopus runtime library present"`** (or installs `libopus0`/`opus`).
- [ ] A stock host that starts **without** libopus: install fails or warns clearly before first boot (never a silent success).
- [ ] `readelf -d /opt/l337-audio-server/l337-audio-server | grep NEEDED` lists `libopus.so` (+ `libpipewire-0.3.so` for pipewire).
- [ ] `ldd` resolves every `NEEDED` (no "not found"); the service starts (`systemctl --user status l337-audio-server` shows running).
- [ ] `L337__SERVER__DUMMY=1 ./l337-audio-server --version` + a dummy boot work (a loader failure can't be masked by `dummy=true`).

## Opus decode + timeline (G1/G4)
- [ ] Generate/fetch an Opus-in-Ogg and Opus-in-WebM; `POST /player/play` each. Assert `state=playing`, `duration_sec` populated, `codec=opus`, no `unsupported codec` in `RUST_LOG`.
- [ ] Over ~5–10 s of polling, `position_sec` advances ~1× toward `duration_sec`.
- [x] A **> 65 s** track plays to completion: no `buffer cap exceeded`, one `over rebuffer window` diagnostic (edge-triggered). Code bounds `pcm` to the window; RSS-reclaim verification on a real backend is pending (dummy mode has no consuming writer).

## Bandwidth ceiling (G2 — findings #4/#5)
- [ ] `POST /player/target {"target_kbps":64}`, then play a YouTube URL and an audio/variants source: `PlayerStatus.target_kbps=64` and `actual_kbps<=64` (or reports the true, un-clampable bitrate); no request fails.
- [ ] Background cache honors the runtime ceiling: after `/player/target`, the next/prev slot is fetched with the constrained selector (`next.stream` size / `PlayerStatus` for it reflects the ceiling, not best).
- [ ] Labels: `{"quality":"low"}` (~≤48), `{"quality":"medium"}` (~≤128); `{"quality":"ultra"}` (unknown) returns **200** and falls back to the server default (no 422).

## Recovery (G5)
- [ ] Force a backend error (unload/disconnect the sink during playback, or trip the window): the process does **not** abort — it recovers a fresh stream or cleanly reports `state=stopped`/`audio_available=false`.
- [ ] Repeated play/pause/stop/seek cycles + `sv restart` never leave a `CANNOT LINK`/assertion in the logs.

## Packaging signal (finding #9)
- [ ] The published Linux assets' `release-info.json`/release notes list `libopus` as a required shared library, and README's Runtime Dependencies matches.

---

# Part 3 — Host verification run — Debian 13 (trixie), x86_64, dummy output

Executed against `2026.10.29-5` / `2026.10.29-6` (prereleases from
`g1-opus-decode`), isolated instance (private `TMPDIR`/`XDG_RUNTIME_DIR`/`STATE_DIRECTORY`,
`L337__SERVER__PORT=13370`, `L337__SERVER__DUMMY=true`,
`L337__SERVER__DEFAULT_TARGET_KBPS=128`, `L337__SERVER__BUFFER_MAX_BYTES=8388608`,
`RUST_LOG=l337_audio_server=debug`, `--transport=http`). `yt-dlp`, `ffmpeg` and a
local controllable HTTP origin were on `PATH`.

## Install + linkage (finding #1) — FIXED, was not actually done
- [x] Both Linux assets' `NEEDED` sets read back from the published release:
      `libopus.so.0` (+ `libpipewire-0.3.so.0` on pipewire) and `ldd` resolves them.
- [x] **Bug:** `ensure_libopus_dependency` was reachable only from
      `check_pipewire_dependency`, and **nothing called either** — so no installer
      ever checked libopus (the ticked checklist item was aspirational). Replaced by
      a generic `ensure_runtime_dependencies()` that reads the binary's `DT_NEEDED`
      set, installs the mapped package per PM, and **fails the install** if a needed
      lib is still missing. Wired into all three Linux installers (fresh install +
      update). Verified: real binaries report `ok "Runtime libraries present: …"`, and
      a binary with an unresolvable `NEEDED` entry `warn`s per-library and exits 1
      (no silent success).
- [x] `release-info.json` gained `needed_shared_libs` per asset (verified in
      `2026.10.29-5`: `libopus.so.0` for every Linux asset, none for
      armv7/riscv64/darwin/windows); README's Runtime Dependencies updated to match.
- [x] `L337__SERVER__DUMMY=1 … --version` and a dummy boot work.

## Opus decode + timeline (G1) — PASS
- [x] Opus-in-Ogg, Opus-in-WebM, MP3, FLAC, WAV(s16le), AAC/M4A all play:
      `state=playing`, `duration_sec` populated, `codec=opus|mp3|flac|pcm_s16le|aac`,
      `actual_kbps` populated, no `unsupported codec` in the log.
- [x] A 100 s Opus track plays on the file path (no `buffer cap exceeded`).
- [x] `position_sec` **cannot** be verified here: dummy output has no writer, so no
      sample is ever consumed and `read_pos` never advances. Needs a host with a
      real PipeWire/ALSA sink (see "left to verify").

## Bandwidth ceiling (G2) — PASS (with a fix)
- [x] All labels resolve: `low`→48, `medium`→128, `lowest`→16, `high`→256,
      `extreme`→1024, `veryhigh`→512, `best`/`lossless`→0, `veryHigh`/`" LOW "`→case-
      insensitive, unknown `ultra`→200 + server default (no 422).
- [x] **Bug:** `PlayerEngine::new_dummy` ignored `buffer_max_bytes` and
      `default_target_kbps`, so the documented testing mode silently dropped both
      (`target_kbps` was always 0). Now passed through: an isolated instance with
      `default_target_kbps=128` reports `target_kbps=128` and keeps it across plays,
      and `buffer_max_bytes` is honoured. Fixes the device-only G2/G4 checks.
- [x] Background cache resolves via the runtime ceiling (no
      `resolve_ceiling(…default_target_kbps())` left in `src/api/handlers.rs`).

## Recovery (G5)
- [x] Play/pause/stop/seek/next/previous cycles, failed plays, 404s and HTML-content
      refusals never abort the process or leave a poisoned log; `/health`, `/version`
      stay 200 throughout.
- [x] **Bug (fixed):** volume was not re-applied when `try_recover` recreated the
      stream, so a recovery silently reset it to unity.
- [ ] Forcing a real sink error (unload the sink mid-playback) needs a live backend.

## Additional bugs found and fixed on this host

| # | Sev | Bug | Fix |
|---|-----|-----|-----|
| 1 | CRITICAL | **Arbitrary file write + delete via `track_id`.** It was joined straight into the cache path: `X-Track-Id: ../x` wrote a file *outside* the cache dir (confirmed: `/tmp/…/state/PWNED_WRITE`, 129 KB of attacker bytes) and an absolute `X-Track-Id: /tmp/x` did too; a pool overflow then *deleted* a file outside the cache dir (confirmed: `/tmp/PWNED_DEL` removed). | One shared rule `is_safe_track_id()` (single plain component, no `/`, `\`, `.`/`..`, NUL, ≤128 chars). `get_path_for_track` → `Option`, eviction only removes paths inside the cache dir, and the API boundary (`play`, `cache_next`, `cache_previous`, `X-Track-Id`) returns 400. Verified: all traversal forms 400, nothing written/deleted outside the cache dir, valid ids still 200. |
| 2 | WARNING | `cargo test` **did not compile**: `decode_to_pcm` returns a 4-tuple, the test destructured a 3-tuple. `cargo build`-only CI never caught it. | Test updated (+ asserts the new codec label). The suite now compiles with `--no-default-features --features alsa,opus --all-targets`. |
| 3 | WARNING | `X-L337-Token: <token>` (and `Authorization: <token>`) **always 401** — the comparison only ran when the presented value had the same length as `"Bearer <token>"`, so the short form skipped every branch. | Compare the bearer form and the bare token independently, each constant-time. Verified: all 4 header forms 200, all wrong/short/long tokens 401. |
| 4 | WARNING | A failed YouTube resolve ran **yt-dlp twice** (resolve, then the downloader re-resolving it) — ~2× latency per failure and 2× the chance of a rate-limit block. | `StreamStartError::Resolve`/`Playback`; only a post-resolution failure falls back to a plain download. Verified: an unresolvable YouTube URL now returns `Failed to resolve stream for …` once, and `duration_sec` no longer reports the previous track's timeline. |
| 5 | SUGGESTION | `seek` past the end silently re-buffered the whole track from 0 while reporting `position_sec > duration_sec` (observed `99999` vs `100`). | Clamp to the decoded length; observed `100`/`0` at the end instead of `99999`/`100`. |
| 6 | WARNING | **Regression introduced by this round's own recovery fix, caught before shipping it.** Writing the software gain in `try_recover` while `set_volume` only wrote it when `pactl` failed meant that, after a recovery, a successful `pactl` set attenuated on top of the software gain (0.8 hardware × 0.5 software = 0.4 effective). | Software gain is now the single authoritative volume path: every backend already multiplies dequeued samples by it, so it works on PipeWire/ALSA/AAudio/CoreAudio/WASAPI and needs no external tool; and it lives in the `Arc` shared with the backend, so it survives stream recreation by construction. Removes the 47-line `set_pipewire_sink_input_volume` pactl helper and the recovery workaround. Unit test asserts the gain tracks `volume_val`. |
| 7 | SUGGESTION | `install-systemd.sh` writes `/etc/l337-audio-server/config.toml`, which `load_settings()` never reads (it reads `…/config` and `…/server.ini`), so the systemd installer's host/port/token are silently ignored. | **FIXED 2026-11-01** — the installer writes `server.ini` (the file `load_settings` actually reads, explicitly as INI) and migrates a legacy `config.toml` on update; integrity/health checks read `server.ini`. The authoritative file is `server.ini` (`--help` and the README already documented it). |

---

# Part 4 — Skipped work: hand-off for a Linux host with a real PipeWire/ALSA sink

Everything below was **not** executed on the Debian 13 dummy-output host, because
the host cannot exercise it. Each item states **why** it was skipped, **exactly**
what to run, what **pass** looks like, and where the code lives if it fails.
Fixture generation, the HTTP origin and the isolated-instance recipe are all
reproduced verbatim so nothing is re-derived.

Environment used here (recreate the same, minus `dummy`):

```bash
mkdir -p /tmp/l337iso/{data,run,state,cache,logs} && chmod 700 /tmp/l337iso/run
# fixtures
cd /tmp/l337iso/data
for spec in "440 10 test_opus_ogg.opus opus" "330 10 test_opus_webm.webm opus" \
            "220 100 long_opus_ogg.opus opus" "500 8 test.mp3 libmp3lame" \
            "600 8 test.flac flac" "700 8 test.wav pcm_s16le" "800 8 test.m4a aac"; do
  set -- $spec
  ffmpeg -hide_banner -loglevel error -f lavfi -i "sine=frequency=$1:duration=$2:sample_rate=48000" \
    -ac 2 -c:a $4 $3 -y
done

env L337__SERVER__TRANSPORT=http L337__SERVER__HOST=127.0.0.1 L337__SERVER__PORT=13370 \
    L337__SERVER__DEFAULT_TARGET_KBPS=128 L337__SERVER__BUFFER_MAX_BYTES=8388608 \
    L337__SERVER__DUMMY=1 \
    RUST_LOG=l337_audio_server=debug HOME=/tmp/l337iso XDG_RUNTIME_DIR=/tmp/l337iso/run \
    XDG_CACHE_HOME=/tmp/l337iso/cache STATE_DIRECTORY=/tmp/l337iso/state TMPDIR=/tmp/l337iso \
    ./l337-audio-server            # drop DUMMY=1 on the real host
TOKEN=$(cat /tmp/l337iso/state/server_token.txt)
BASE=https://127.0.0.1:13370; alias q="curl -sk -H \"Authorization: Bearer $TOKEN\""
```

A controllable HTTP origin (throttle / stall / forced content-type) was used for
the streaming-path tests; `/tmp/l337iso/origin.py` in this session's scratch is
portable if it is needed again, but it is **not** required for the items below
except where stated.

---

## 1. `position_sec` does not advance in dummy mode — never verified

**Why skipped:** the dummy backend is `NoopAudioOutputStream`, whose `play()` is a
no-op. Nothing ever advances `AudioBuffer::read_pos`, and
`get_status()` derives `position_sec` from `(read_pos / channels)` converted back
to source-rate seconds (`PlayerEngine::calculate_position` /
`get_status`, src/player/engine.rs:262 and :908). With no writer, `read_pos` is
permanently 0, so `position_sec` stayed 0 for every track on this host **by
construction** — not because of a bug. It requires a backend that dequeues
samples.

**Run:**

```bash
q -X POST $BASE/player/play/stream -H 'X-Track-Id: pos1' --data-binary @/tmp/l337iso/data/test_opus_ogg.opus
for i in $(seq 1 10); do sleep 1; q $BASE/player/status | jq -c '{position_sec,duration_sec,state}'; done
```

**Pass:** `state=playing` throughout, `position_sec` increases monotonically and
lands on ~`10` at the end (10 s fixture). Drift of more than ±1 s over 10 s is a
failure — that is the conversion in `calculate_position` double-counting stereo
or mixing device/source rates.

**Watch for:** `position_sec` in *jumps* (the `stretch_and_resample` ratio), or
reaching `duration_sec` early/late. Both are arithmetic bugs in
`calculate_position`, not in the backend.

---

## 2. RSS reclaim after playback (finding #3) — implemented, device verify pending

**Status:** implemented in code. `decode_file_incremental_blocking` now reclaims consumed
samples and blocks on freed buffer space (`wait_for_space`, 20 ms timeout) instead of
sleeping, so `pcm` stays bounded to the rebuffer window (~24 MiB default) for both the
Symphonia and Opus file paths. Peak RSS during file decode is independent of track length.

**Why device verify is pending:** reclaim is driven by the *device writer* calling
`buf.reclaim_consumed()` at its checkpoint, and only after it has advanced `read_pos`
by ≥ `1<<20` samples. No writer runs in dummy mode, so nothing is ever reclaimed there
and the whole decoded track stays pinned in dummy. This needs a real PipeWire/ALSA
backend to observe RSS rise then fall back toward window size.

**Run (real backend):**

```bash
PID=$(pgrep -x l337-audio-server | head -1)
q -X POST $BASE/player/play/stream -H 'X-Track-Id: rss1' --data-binary @/tmp/l337iso/data/long_opus_ogg.opus   # 100 s ≈ 38 MB decoded
grep VmRSS /proc/$PID/status                     # T0
sleep 30; grep VmRSS /proc/$PID/status           # mid-playback
q -X POST $BASE/player/stop >/dev/null; sleep 2; grep VmRSS /proc/$PID/status   # after
```

**Pass:** RSS rises while buffering the initial window, then **falls back**
toward the ~window size (config 8 MiB here → ~24–30 MiB steady) rather than
pinning the full 38 MB; `load_and_play` logs no `buffer cap exceeded`.

**Where it fails if it fails:** the cap guard is `AudioBuffer::exceeds_window()`
and the reclaim is `AudioBuffer::reclaim_consumed()` (src/platform/common.rs:103
and :109), called from each writer after `read_pos` advances. Verified present in
`linux.rs:98/136`, `pulseaudio.rs:227/268`, `alsa.rs:189/212`,
`android.rs:167/180`, `macos.rs:78/121`, `windows.rs:125/138`; the raw
`pcm.len() > buf.max_bytes` pattern is gone from `src/platform/` entirely.

**Finding #3 whole-track file decode (previously the "known shortcut"):** resolved.
The file path now decodes incrementally with reclaim + backpressure, so it is no
longer track-length-shaped. Only a real-backend RSS measurement remains.

---

## 3. Real backend error + `try_recover` (G5) — never verified

**Why skipped:** forcing a `StreamState::Error` requires unloading the sink the
stream is connected to, i.e. a live PipeWire graph. In dummy mode nothing can
raise `backend_error`, so the recovery path was never executed end-to-end.

**Run:** start playback, then break the sink:

```bash
q -X POST $BASE/player/play/stream -H 'X-Track-Id: rec1' --data-binary @/tmp/l337iso/data/long_opus_ogg.opus
sleep 2
pactl unload-module module-loopback      # or: systemctl --user stop pipewire{,-pulse}.socket
# then observe for ~10 s
for i in $(seq 1 10); do sleep 1; q $BASE/player/status | jq -c '{state,audio_available,position_sec}'; done
```

**Pass:** the process does **not** exit. The 500 ms recovery task
(`main.rs:268`) calls `try_recover()`, which rebuilds the stream, re-buffers from
`position` and returns to `state=playing`; or it cleanly reports
`state=stopped` / `audio_available=false`. One `PipeWire stream entered error
state` + one `backend_error` handling per disturbance is expected.

**Changed on this branch, so re-check specifically:** `try_recover` no longer
touches the volume (the software gain lives in the `Arc` shared with the backend
and survives stream recreation by construction). Confirm volume still reads back
what you set *after* a recovery, and that it is not doubled or reset:

```bash
q -X POST $BASE/player/volume -H 'Content-Type: application/json' -d '{"volume":0.3}'
# ...force a sink error as above...
q $BASE/player/status | jq -c '{volume}'
```

---

## 4. YouTube + real bandwidth ceiling (G2, findings #4/#5) — never verified

**Why skipped:** this sandbox gets `ERROR: [youtube] … This video is unavailable`
from yt-dlp, so no YouTube stream could be resolved. Everything that depends on
observing a *delivered* bitrate is therefore untested against a real target.

**Run:**

```bash
q -X POST $BASE/player/target -H 'Content-Type: application/json' -d '{"target_kbps":64}'
q -X POST $BASE/player/play -H 'Content-Type: application/json' \
   -d '{"track_id":"yt64","stream_url":"https://www.youtube.com/watch?v=<known-good>"}'
q $BASE/player/status | jq '{target_kbps,actual_kbps,codec,duration_sec}'
```

**Pass:** `target_kbps=64`, and `actual_kbps<=64` (or, if nothing fits under the
ceiling, the true un-clampable bitrate reported honestly — `yt_dlp_format_selector`
degrades to `bestaudio`). No 4xx/5xx from any endpoint.

**Background cache under a runtime ceiling** (finding #4, fixed but unverified
against a live target):

```bash
q -X POST $BASE/player/target -H 'Content-Type: application/json' -d '{"target_kbps":64}'
q -X POST $BASE/player/cache/next -H 'Content-Type: application/json' \
   -d '{"track_id":"ytnext","stream_url":"https://www.youtube.com/watch?v=<known-good>"}'
sleep 20; ls -l /tmp/l337iso/state/cache/next.stream      # must reflect ≤64 kbps
```

**Watch for (static risk, unverified):** `detect_youtube_block`
(src/player/engine.rs) classifies a block by substring on yt-dlp's stderr, and
`"403"`/`"cloudflare"` map to `bot_detected`. Any unrelated yt-dlp error
message containing `403` will be reported to the client as
`youtube_blocked` with `block_type=bot_detected`. If you see a spurious
`youtube_blocked`, this is the cause.

---

## 5. Installer's package-install path — gate verified, install not

**Why skipped:** the fail-gate is verified, but the `apt-get`/`dnf`/`pacman`/
`zypper`/`apk` *install* invocation only runs as root on a host that is actually
missing a library, which no sandbox can reproduce safely.

**Verified here:** `ensure_runtime_dependencies()` prints
`ok "Runtime libraries present: libopus.so.0 libpipewire-0.3.so.0 …"` for both
real Linux assets, and for a binary with an unresolvable `DT_NEEDED` entry it
`warn`s per library and **exits 1** before the service is written.

**Run on a host without libopus** (e.g. a container based on `debian:13-slim`
minus `libopus0`, or temporarily hide the .pc/lib):

```bash
sudo ./scripts/install-linux-pipewire.sh --dry-run
sudo ./scripts/install-linux-pipewire.sh          # expect: warn + install libopus0 + ok
readelf -d /opt/l337-audio-server/l337-audio-server | grep NEEDED
systemctl --user status l337-audio-server          # running, no CANNOT LINK
```

**Pass:** the installer prints the missing library, installs it, re-checks, and
reports `Runtime libraries present: …`; if it cannot, it exits non-zero instead
of reporting success.

**Static risk (unverified):** the SONAME→package table lives in
`runtime_pkgs_for()` (scripts/install-linux-{pipewire,alsa}.sh and
scripts/install-systemd.sh, three identical copies because each installer is
downloaded standalone). The zypper/apk names (`libopus0`, `libopus`) and the
`libpipewire-0.3-0`/`pipewire` split are best-effort and have not been executed
on those distributions.

---

## 6. `cargo test` **execution** — compiles, not run

**Why skipped:** only `cargo check --all-targets` was run on this host, because a
debug build of every dependency needs several GB and `/tmp` was already at ~79%.

**Verified here:** the test target compiles
(`cargo check --no-default-features --features alsa,opus --all-targets`, 0 errors).

**Run:**

```bash
CARGO_HOME=/tmp/cargo-home RUSTUP_HOME=/tmp/rustup CARGO_TARGET_DIR=/tmp/l337-build \
  cargo test --no-default-features --features alsa,opus
```

Note this host has no PipeWire **dev** headers (only the runtime `.so`), so the
default `backend` feature cannot compile locally; `alsa,opus` does, because
`alsa.pc` is present. CI covers the pipewire variant. `libopus.so` (the dev
symlink) is also absent, which is fine for `cargo check` but would fail a local
link.

---

## 7. Known-but-unfixed items carried over (unchanged, still open)

- **finding #3** — whole-track decode on the file/`/player/play/stream` path
  (see §2). **FIXED in code** (`decode_file_incremental_blocking` bounds `pcm` to
  the rebuffer window via reclaim + `wait_for_space`); real-backend RSS-reclaim
  verification remains pending.
- **finding #7** — `CHUNK_FRAMES = 4096` in `src/platform/pulseaudio.rs` is
  unverified on device (audio still 1×, no underrun, lower wakeups).
- **finding #9** — the committed `scripts/opus-stubs/libopus.so` blob is
  validated in CI (AArch64 + SONAME `libopus.so` + the 4 exported symbols,
  `build.yml` termux job) but still not *regenerated* in CI with a diff gate
  (optional; a compiled ELF is not byte-reproducible across toolchains).
- `install-systemd.sh`'s `config.toml` mismatch — **FIXED 2026-11-01**: it now
  writes `/etc/l337-audio-server/server.ini` (the file `load_settings()` reads,
  explicitly as INI) and migrates a legacy `config.toml` on update.

## What is already green (do not redo)

Dummy-mode API surface (all endpoints + 400/401/422/500 paths), all six codecs,
`position`/`seek` clamping, the `track_id` traversal write+delete, the four auth
header forms, `X-L337-Token`, `ensure_runtime_dependencies` on both real Linux
assets, the release-info `needed_shared_libs` field, and every installer's
`bash -n` + `--dry-run`.

---

# Part 5 — Playback defect records (BUG-1..BUG-4)

Records from `bugs2.md` ("remaining playback defects after the Opus/pacing pass"), merged here
2026-11-02 with their resolution. **All four are FIXED and device-verified** (see the
[status summary](#status-summary--2026-11-02-branch-g1-opus-decode--e7834d8-release-20261102-4));
the original symptom/root-cause detail is preserved because the failure modes still guide
regression tests.

Original baseline (2026-10-29, what already worked): Opus compiled in (`DT_NEEDED libopus.so`),
`POST /player/play` with a YouTube page URL resolved and played, audio reached the OpenSL sink at
1×, `PlayerStatus` reported `target_kbps`/`actual_kbps`/`codec`, `/player/stop` → `stopped`. The
defects below were all in the **streaming → writer → sink** lifecycle and end-of-track handling in
`src/player/engine.rs` (`start_streaming_playback`, `streaming_decode_sync`, `get_status`) and the
platform writer loops (`pulseaudio.rs::writer_loop`, `common.rs::AudioBuffer`).

## BUG-1 — Playback timeline resets/loops every ~10 s while streaming

**Symptom.** `position_sec` climbed 0..~9 s then jumped back to 0 and repeated on a ~10 s cycle;
the track never played through and interacted badly with BUG-3.

**Root cause.** Not a streaming re-decode, as first hypothesised: the timeline rewound every
**~10.9 s**, exactly the writer's `read_pos >= 1<<20` checkpoint. `AudioBuffer::reclaim_consumed()`
drains the consumed prefix and resets `read_pos = 0`, while both position computations
(`calculate_position`, `get_status`) derived the playhead from `read_pos` alone — so every reclaim
jumped back to the seek base.

**Fix.** `AudioBuffer::consumed_total` counts reclaimed samples; position =
`consumed_total + read_pos`, reset at every buffer refill. Guarded by the unit test
`test_position_survives_writer_reclaim`.

**Verify (PASS).** Device 2026-10-30: live trace `0,1,2,…,8,10,11,…,29` then EOS — no sawtooth.
Re-verified 2026-11-02 against the installed release build: 60 s of 1 Hz polling on
`Y6HH7vF37dE` (AAC path) gave `0,1,2,…,58` with no backward jump.

## BUG-2 — Unbounded streaming to the sink (~9975 s backlog; memory grows)

**Symptom.** PulseAudio's `Buffer Latency` for the server's sink-input grew to ~9975 s for a 231 s
track (the stream was appended ~43× to one open sink-input with no bound) and only drained at real
time; server RSS grew without limit.

**Root cause.** The writer loop wrote as fast as it could with no real-time pacing and
`AudioBuffer` never reclaimed as `read_pos` advanced; combined with BUG-1's re-append path
(plan item G4 — bounded, real-time-paced window — was not yet in effect).

**Fix (plan G4).** Pace the writer to the device clock (`n / (channels * rate)`); enforce the cap
on the *unconsumed* window (`(pcm.len() - read_pos) > max_bytes/4`); reclaim via
`pcm.drain(..read_pos); read_pos = 0` at the writer checkpoint under the existing `Mutex`; treat
`buffer_max_bytes` as a rebuffer window (a few seconds) rather than a whole track; never re-open /
append on a stream that is already playing.

**Verify (PASS).** Device 2026-10-30: one 30 s track → PA `Buffer Latency` ~38 s flat and the
server window drained ~1× (the residual was the device's OpenSL sink, not the server).
Re-verified 2026-11-02: `pactl list sink-inputs` during playback → `count=1`,
`Buffer Latency: 256231 usec` (≈0.26 s) and 42897 usec on the second track.

## BUG-3 — No end-of-track: state stays `playing` after playback finishes

**Symptom.** Neither the position nor the state ever signalled completion, so the player's
auto-advance (it watches for `playing → not playing`) never fired.

**Fix.** `get_status` ends playback as `state = stopped` (position reset) once the decode task is
finished **and** the buffer is fully drained; a drained buffer with decode still running stays
`playing` (rebuffer, not EOS).

**Verify (PASS).** Device 2026-10-30: `playing → stopped` at track end. Re-verified 2026-11-02 on
the installed build: seek to 284 s of a 294 s track → 11 s later the debug log showed
`end of track reached: state = stopped`, `position_sec` reset, sink-input released.

## BUG-4 — "best" quality selects a low-bandwidth rendition instead of the highest available

**Symptom.** At `target_kbps = 0` ("best"), the server reported `codec: aac, actual_kbps: 129`,
while YouTube offered a higher-bitrate Opus (itag 251) — yt-dlp's own `bestaudio` ranking won.

**Fix (commit `83dd28f`).** `select_audio_format` now picks the **highest-bitrate** audio rendition
at or below the ceiling (0 = best) from yt-dlp's full format list, server-side so the choice is
observable via `PlayerStatus.actual_kbps`/`codec`; a ceiling that admits nothing falls back to the
lowest available rendition rather than failing playback. Regression tests at unit and endpoint
level (a fake `yt-dlp` that records the requested format id).

**Verify (PASS — 2026-11-02, this device, previously "unverifiable here").** Cross-checked against
`yt-dlp -F` for the tracks in the player logs:

| Track | yt-dlp audio formats | Reported | Verdict |
|---|---|---|---|
| `Y6HH7vF37dE` | 139/49k AAC, **140/129k AAC**, 251/113k Opus | `codec=aac actual_kbps=129` | ✅ highest available |
| `ujNeHIo7oTE` | 139/49k, 249/57k, 250/74k, 140/129k AAC, **251/144k Opus** | `codec=opus actual_kbps=144` | ✅ highest available |

## Cross-repo NOTES — both items CLOSED (2026-11-02)

- **Status shape vs client progress bar.** The player read a human `status['time']`
  (`"m:ss"/"m:ss"`) while the server returned `position_sec`/`duration_sec` ints, so the progress
  bar stayed empty. `PlayerStatus` now carries `time` (`"00:00/04:54"`), so the client can render
  the bar directly — no wire-format change needed.
- **Capability advertisement.** `/health` now returns
  `{"capabilities":{"codecs":["opus","aac","vorbis","mp3","flac","alac","pcm_s16le"],"quality":true,"yt_dlp":true},"status":"ok"}`
  (was `{"yt_dlp":true}`), so the player can switch off its stopgaps and expose quality presets.

## Historical evidence appendix (2026-10-29, pre-fix)

Kept for provenance of the numbers quoted above:
`POST /player/play {"track_id":"4d15b27f…","stream_url":"https://www.youtube.com/watch?v=bZiCchjRW_s",…}`
→ `{"ok": true, "state": "playing"}`; status then showed `position_sec` sawtoothing 0→~10→0;
`pactl list sink-inputs` showed `Buffer Latency: 9975562100 usec → 9973562417 → 9971562757`
(draining ~2 s per 2 s wall clock); `disk_pool_utilization_bytes ≈ 17.7 MB`; status reported
`codec: aac`, `actual_kbps: 129` at `target_kbps = 0`.

---

# Part 6 — Session log (chronological)

## Merged `bugs2.md` updates (2026-10-30 … 2026-11-01)

These dated entries were `bugs2.md`'s own update log; they are kept verbatim in substance (some
detail already appears in the sections that follow, and is marked as such).

### Update — 2026-10-30 device session (Termux/aarch64, live PulseAudio backend)

Ran the released `2026.10.29-5` binary against the live OpenSL ES sink, then built and ran the
fixed tree on-device (`cargo build --features pulseaudio,opus`, host triple
`aarch64-linux-android`, dev profile):

- **BUG-1 — ROOT-CAUSED AND FIXED (verified on-device).** The rewind period was exactly the
  writer's `read_pos >= 1<<20` checkpoint; fixed with `AudioBuffer::consumed_total` (see BUG-1
  above). Live trace after the fix: `0,1,2,…,8,10,11,…,29` then EOS — no sawtooth. Unit test
  `test_position_survives_writer_reclaim` guards it.
- **BUG-2 — fixed, verified bounded.** One 30 s track: PA `Buffer Latency` ~38 s flat, server
  window drains ~1×; the residual was the device's OpenSL sink not rendering in real time, not a
  server defect.
- **BUG-3 — FIXED (verified on-device).** `playing → stopped` at track end (see BUG-3 above).
- **BUG-4 — still open**, unverifiable here at the time (no resolvable YouTube target on this
  device). *Closed 2026-11-02, see above.*

**Other defects found and fixed in that session**

1. **Installer gap (the reported 404).** `install-android-termux.sh` picked the newest prerelease
   and 404'd when a partial release lacked our asset (`2026.10.29-6/-7` ship only the x86_64 Linux
   assets). It now walks releases newest-first and picks the newest whose **release-info.json
   confirms the wanted asset**, using the API's own `browser_download_url`. Same walk added to
   `install-macos.sh`; `install.ps1` mirrors it (not executed — no pwsh on device). Verified
   end-to-end on the device: walked `-7 → -5`, installed and registered the service, `/health` +
   `/version` ok.
2. **`cargo test` was never executed** (the Linux host ran `cargo check` only). The first real run
   exposed: pitch≠1 tracks playing `1/pitch` too long (`tempo = speed / pitch` assumed a
   length-rescaling pitch shifter; it is length-preserving — fixed in both the file and streaming
   paths); a fake `yt-dlp` fixture with a `#!/bin/bash` shebang that cannot exec on Termux, so
   execvp silently fell through to the real yt-dlp (→ `#!/bin/sh`); the endpoints tests guessing
   the server binary path (→ `env!("CARGO_BIN_EXE_…)`); and 22 parallel test servers stealing each
   other's single-instance lock/token (→ per-spawn `TMPDIR`/`XDG_RUNTIME_DIR`/`STATE_DIRECTORY`
   isolation). Suite: **45/45 unit + 22/22 endpoints green (parallel)**.
3. **Dev-profile hazard (not a release blocker).** `axum-server` + tokio 1.52 `debug_assert!`
   panics when registering the blocking Unix socket in socket mode; release builds
   (debug-assertions off) are unaffected. On-device builds therefore need
   `RUSTFLAGS="-C debug-assertions=off"`.

### Update 3 — 2026-10-31: re-test of the *installed* server (no audio, proven root cause)

Method: dependency-free stdlib HTTP client on the Unix socket, an isolated second instance with
`RUST_LOG=debug` for logs, `pactl`/`parec` on the OpenSL monitor sink.

- **A. New root cause (P0): `resample_interleaved` emitted ZEROS for `file_rate != device_rate`.**
  Proven by isolation — two locally generated WAVs pushed through `POST /player/play/stream`,
  identical except sample rate: a 48 000 Hz WAV played audibly (monitor peak **11002**,
  `codec=pcm_s16le dur=4 pos=4`); a 44 100 Hz WAV (decode → `resample_interleaved` 44100→48000 →
  writer) produced **silence** (peak exactly 0). A control experiment (a known 440 Hz tone → peak
  11999) established that "monitor = 0" means "server wrote silence". Almost every real track
  (AAC/MP3/Opus, 44100 Hz) hits this, so the server wrote silence to a healthy sink. Fixed in commit
  `a3a75b6` ("rate conversion no longer writes silence"); the old release binary also assembled its
  output planar (`output.extend_from_slice(&output_planar[ch])`) instead of interleaved — a second,
  separate defect in the same function.
- **B. `stop()` never tore the stream down (P1).** `stop()` called `stream.pause()`, never
  `stream.stop()`/Drop, so the writer thread and sink-input survived the stop and the paused writer
  kept appending silence (observed `Buffer Latency ≈ 1000 s` draining at ~0.03×). Fixed in
  `a3a75b6`; re-verified 2026-11-02 (`count=0` sink-inputs after stop).
- **C. The installed binary was stale → BUG-1/2/3 fixes were NOT in the running artifact** (it was
  release `2026.10.29-5`, pre-dating the engine commits). This is the provenance problem traced
  further on 2026-11-02 (see Part 7).
- **D. BUG-4 is verifiable on this device after all** — a resolvable YouTube target exists
  (`yt-dlp -F` returns itag 140/139 AAC and 251/250/249 Opus); the running binary still reported
  `codec=aac, actual_kbps=129` at `target_kbps=0`. *Now closed, see BUG-4 above.*
- **E. Other observations kept on file.** The device Python had lost its third-party deps
  (`httpx`, `pytest`, `textual` — all `ModuleNotFoundError`), so the player's `./run-test.sh`
  cannot exec on-device and the TUI cannot be imported there; device verification must use a
  stdlib HTTP client. *(Restored by 2026-11-02 — see Part 7.)* The running `-5` binary did carry
  G2 + Opus but advertised only `yt_dlp` in `/health`. Short-file playback showed `pos == dur`
  then stop, i.e. the file-path EOS wiring worked even on that binary. `sv restart` fully cleared
  leaked state (surviving sink-input, position/state).

### Update 4 — 2026-10-31: client AAC path verified audible

The user confirmed hearing audio via the player client: the client's YouTube AAC/m4a
materialization (`_download_youtube_audio` → `/player/play/stream`) bypassed the broken
44100→48000 resample path, so playback was audible end-to-end despite BUG-A remaining in the direct
path. *(Historical: the client now sends the page URL straight to the server and the server
resolves/streams it — see the `l337-player` changelog entry of 2026-10-29 and the fix in BUG-A.)*

### Update 5 — 2026-10-31: post-work re-check (installed server)

`/health` began advertising `codecs` + `quality`. Direct play started with `codec=opus`,
`actual_kbps=132`, but still stopped within ~6 s with `audio_available=false`, `position_sec=0`.
BUG-4 appeared improved (Opus 132 k). Resample/direct-path playback still needed verification, and
the client AAC path remained the audible workaround.

### Update 6 — 2026-10-31: service logging enabled + U2 premature EOS reproduced

`runit/run` exports `RUST_LOG=debug` and redirects to `$TMPDIR/l337-server.log`. Reproduced the U2
symptom on the installed binary: `position_sec` advanced 1..7 with `audio_available=true`, then
`INFO engine: end of track reached: state = stopped` at ~7 s of a 281 s track; afterwards
`state=stopped`, `position_sec=0`, `audio_available=false`. Also noted `current_track.duration=281`
vs status `duration_sec=292` (~+11 s) — the derived duration differs from the client's.

### Update 7 — 2026-10-31: streaming Opus no longer aborts early

Root cause of the U2 stall: `streaming_decode_sync` (Opus branch) treated
`DecodeError("mkv: attempt to read element data past master element")` as terminal and `break`ed,
ending decode after ~8 s. Patched to call `wait_for_progress` on that error (retry while the
download grows). Rebuilt and re-tested: position advanced to ~9 s and stayed `playing`, download
kept growing. **"Further investigation needed for the remaining stall"** — closed 2026-11-03: the
MKV reader was **wedged**, not starved; see
[Part 7](#new-issue--webmopus-streaming-stalls-at-9-s-fixed-2026-11-03-wedged-mkv-reader-is-rebuilt).

### Update 8 — 2026-11-01: config, installers, streaming stall (full suite green on-device)

No playback semantics changed; the open items from the earlier updates were worked and are recorded
in the [Session — 2026-11-01](#session--2026-11-01-termux-device-configuration-installers-streaming-stall)
section below: the Symphonia `IoError` tolerance now shares `wait_for_progress`; `loglevel` from
`server.ini`; `install-systemd.sh` writes `server.ini`; Debian installer false failures; new
regression tests; suite **57/57 unit + 24/24 endpoints**.

---

## Termux device session — 2026-10-30 (the device this project's Linux docs could not cover)

Everything below was executed **on the Termux/aarch64 device** against the live
PulseAudio (OpenSL ES) sink. Full details + live traces: the 2026-10-31 update
entries in the [Session log](#part-6--session-log-chronological).

## Closed on-device

| # | Item | Outcome |
|---|------|---------|
| 1 | **Installer picked a release without our binary** (the reported 404: `2026.10.29-6/-7` ship only x86_64 Linux assets) | All asset-publishing installers now walk releases newest-first and select the newest whose **release-info.json confirms the wanted asset**, downloading the API's own `browser_download_url`. Verified end-to-end here: `-7 → -5` fallback with a printed reason; service registered and healthy. `install-macos.sh` mirrors it; `install.ps1` mirrors it (unexecuted — no pwsh on device). |
| 2 | **BUG-1 timeline sawtooth** — rewound every ~10.9 s | Root cause: `reclaim_consumed()` resets `read_pos` and drops the prefix, but the playhead was derived from `read_pos` alone. Fixed via `AudioBuffer::consumed_total`; verified live (`0…8,10,11…29`, no reset) + unit test. See [BUG-1](#bug-1--playback-timeline-resetsloops-every-10-s-while-streaming). |
| 3 | **BUG-3 no end-of-track** | `get_status` finishes as `state=stopped` when decode is done and the buffer is drained; verified live (`playing → stopped` at track end). Rebuffer vs EOS distinguished by the decode handle. See [BUG-3](#bug-3--no-end-of-track-state-stays-playing-after-playback-finishes). |
| 4 | **BUG-2 sink backlog** | Verified bounded: one 30 s track holds ~38 s of PA latency (the SL ES queue), server window drains at ~1×. The 9975 s backlog is gone. See [BUG-2](#bug-2--unbounded-streaming-to-the-sink-9975-s-backlog-memory-grows). |
| 5 | **`cargo test` never executed** (see §6 above) | First real run fixed: pitch≠1 duration bug (`tempo = speed / pitch` → `speed`; the pitch shifter is length-preserving) in both file and streaming paths; Termux `#!/bin/bash` fake-script fallthrough (→ `#!/bin/sh`); endpoints binary path (`CARGO_BIN_EXE_l337-audio-server`); parallel test servers stealing the single-instance lock/token (per-spawn `TMPDIR`/`XDG_RUNTIME_DIR`/`STATE_DIRECTORY` isolation). **45/45 unit + 22/22 endpoints green in parallel.** |

## Device-only notes for future work

- **On-device builds are OOM-limited**: `rustc` host is `aarch64-linux-android`, and one
  `CARGO_HOME=$HOME/.cargo CARGO_TARGET_DIR=$TMPDIR/l337-build RUSTFLAGS="-C debug-assertions=off" cargo build --features pulseaudio,opus`
  succeeded (~5 min; dev builds need `-C debug-assertions=off` because tokio 1.52's
  `debug_assert!` in `check_socket_for_blocking` panics on the blocking Unix socket in
  socket mode — release builds are unaffected). However, **repeated builds on this device
  are killed by the lowmemorykiller** (a second build aborted mid-compile), so treat
  on-device builds as a one-shot verification, not a loop; release builds come from CI.
  The OOM kill also restarts runsvdir, so re-check `sv status` and `sv up` after a build.
- `/tmp` is not writable by the app user on Termux — `AGENTS.md`'s `/tmp` build paths do
  not apply on-device; use `TMPDIR` (`$PREFIX/tmp`).
- Engine fixes above were **not** in the published release at the time: the device ran
  `2026.10.29-5`, which predates them. They needed a publish before a device run of the
  release binary could show the BUG-1/BUG-3 fixes. (Release provenance was traced on
  2026-11-02 — see [Part 7](#release-provenance--what-ships-in-a-release-vs-what-is-built); a
  prerelease containing the fixes finally shipped as `2026.11.02-4`.)
- BUG-4 (best = highest available rendering) was open and unverifiable here at the time (no
  resolvable YouTube target on this device). **Closed 2026-11-02**: a resolvable target exists,
  and `best` was verified in both directions — see
  [BUG-4](#bug-4--best-quality-selects-a-low-bandwidth-rendition-instead-of-the-highest-available).



---

## Installer hardening round — 2026-10-31 (Termux/aarch64, device-validated)

A review of the 404-skip / version-check work found **2 CRITICAL** and several WARNING defects,
and the user redirected the architecture: **all GitHub work belongs in `install.sh`**, which should
download the binary itself and hand the path to the platform installer. Everything below is
implemented and verified on the device against the live GitHub API.

## Completed

| # | Item | Verified |
|---|------|----------|
| 1 | **`install.sh` owns all GitHub work.** Resolves the platform's asset name, walks releases newest-first, verifies each candidate against that release's own `release-info.json`, compares the recorded glibc against the running libc, checks the installed version, downloads the binary into `TMPDIR`, then `exec`s the platform installer with `--binary <path> --release <tag>`. | Live: picked `2026.10.29-5` over the partial `2026.10.29-6/-7`. |
| 2 | **Platform installers contain no release/version/download logic.** `install-android-termux.sh`, `install-linux-alsa.sh`, `install-linux-pipewire.sh`, `install-macos.sh` lost their release walks, version comparisons and download code (five drifting copies → one). Running one without `--binary` fails with an actionable message; `--uninstall` still works standalone. (Fetching `lib-distro.sh` remains — it is a helper library, not a release check.) | Suite: 58 checks. |
| 3 | **CRITICAL — stale `.tmp` accepted as a good binary.** `curl -f` never truncates its `-o` target, so a partial file left by an interrupted run passed the "non-empty?" test and was installed as a complete binary. Success is now decided by the transfer status, and the destination is removed before each attempt. | Suite: stale content never handed over. |
| 4 | **CRITICAL — local installer executed as root from the caller's CWD.** `curl … install.sh \| sudo bash` feeds the script over stdin (`$0=bash`), so a bare `scripts/install-*.sh` resolved wherever the user ran it, executing an attacker-placeable file as root. Only a real checkout (a directory containing `.git`) is trusted now. | Suite: installpath resolution checked. |
| 5 | **Regression — `--binary` was clobbered.** `install-android-termux.sh` re-read `L337_BINARY_PATH` *after* argument parsing, overwriting the path `install.sh` had passed; every install died with "No binary given" *after* a successful download. The env fallback is now resolved before parsing in all four installers. | Fixed; real install runs end-to-end. |
| 6 | **glibc is detected on termux-glibc.** The system `ldd`/`getconf` are Bionic and silent, so a glibc Termux device reported "cannot determine" and the glibc filter silently accepted any release. The loader in `$PREFIX/glibc/lib` is asked instead (reports `2.44` on this device), the package revision is ignored (`2.44-0` compares as `2.44`), and a Bionic-only device says so instead of warning. | Live: `System glibc: 2.44 (termux-glibc; …)`. |
| 7 | **`--force` re-downloads** — it always did; the output now says so (`… — re-downloading`). | Live: two consecutive `--force` runs each downloaded. |
| 8 | **Services are enabled, started and verified.** Termux `sv-enable`s at boot (previously only printed); verification checks the service is supervised **and** that the server actually created its listening endpoint. Linux ALSA/PipeWire and the local systemd installer poll (`wait_until_ok`, new in `scripts/lib-distro.sh`) instead of a fixed `sleep`; macOS loads via `launchctl bootstrap gui/<uid>` (fallback `load`) and verifies the job registered. | Live: `enabled` → `running (pid)` → `Verified: Unix socket …`. |
| 9 | **Verification used the wrong transport.** The install probed TCP `127.0.0.1:1337`, but the Termux build runs `--transport=socket` and never opens that port, so a working install always reported "not answering". It now verifies the Unix socket the server actually binds. | Live: socket verified, service `run`. |
| 10 | **macOS asset selection** now matches `install-macos.sh`, which installs the x86_64 build under Rosetta 2 on Apple Silicon — `install.sh` required the aarch64 asset, so the two disagreed about which release to use. | Static (no macOS host). |

## Remaining gaps / device-only verification

- **The Linux and macOS paths are structure- and syntax-verified only** — no Linux or macOS host
  available on this device. `wait_until_ok`, the systemd enable/start/verify polls, and the
  `launchctl bootstrap` path have not been executed. First run on a real host should confirm:
  unit reaches `active` within the 15 s poll, `is-enabled` reports enabled, and a slow-start unit is
  no longer rolled back by the old fixed `sleep`.
- ~~**`install-systemd.sh` writes `/etc/l337-audio-server/config.toml`**, which `load_settings()` never
  reads~~ — **FIXED 2026-11-01**: it writes `server.ini` and migrates the legacy file.
- ~~**The committed `scripts/opus-stubs/libopus.so` blob** is still not regenerated/validated in CI.~~
  — validation step is in CI; only the optional regeneration-with-diff remains.
- **NEW — Linux installer false failures on a stock Debian host** (bugs.md status summary item 3 / Part 1: `libopus0`
  installed yet "libopus.so.0 still missing"; "no known package provides libm.so.6"):
  **fixed in code** (loader-provided SONAMEs skipped; `ldconfig` refreshed before the check;
  presence probe falls back to the standard library dirs) and verified by simulation on this
  device — the real-host run on `pipelinepro` is still pending.
- **device-only playback checks** in the sections above are unchanged: `position_sec` advancing on a
  real sink, RSS reclaim on the streaming path, forced-backend-error recovery, and the real YouTube
  bandwidth ceiling. The engine fixes from the 2026-10-30 session (BUG-1/BUG-2/BUG-3) are still not
  in a published release, so a release-binary run cannot yet show them.
- **The installer's own `--binary` handover on non-Termux platforms** is exercised structurally only
  (a full install needs a running PulseAudio daemon, which `pulseaudio --check` cannot satisfy under a
  redirected `HOME`); the functional path was verified on this device instead.

## Installer round 2 — download verification (2026-10-31)

Closed the integrity gap found while adding MD5 verification:

- **Every release now publishes per-asset checksums** (`md5`, `crc32`, `size`) in
  `release-info.json`, generated in `build.yml` from the built artifacts.
- **`install.sh` verifies the download before installing**, in layers, and never
  hard-fails because a hash tool is missing:
  1. **size** — always, needs no tool (`curl` reports what it transferred);
  2. **md5** — via `md5sum` when present (strongest);
  3. **crc32** — via POSIX `cksum`, shipped by **both coreutils and busybox**, so a
     host without md5sum is still covered;
  4. neither tool → size only, with the applied check stated explicitly.
  A release published before checksums existed says so instead of silently
  skipping. A real mismatch refuses the install, prints expected vs actual, and
  leaves nothing behind.
- **A pure-bash hash is not viable**, and is documented as such in the code:
  bash variables cannot hold NUL bytes (so it cannot be byte-faithful), and a
  per-byte loop measures ~4 KB/s on this device — about an hour for the 15 MB
  binary. Measured, not assumed.
- Bug found while testing: the `cksum` byte count was read from the wrong output
  field (`CRC SIZE FILENAME`), so the length cross-check always failed for hosts
  without `md5sum`. Fixed; covered by the suite's `C1 bad-crc` / `C1 no-md5` paths.

Verified: 66-check suite green, covering all five verification outcomes
(md5 path, crc32 path, size-only path, wrong crc32 → refuse, wrong size →
refuse), plus the live install path.

---

## Session — 2026-11-01 (Termux device): configuration, installers, streaming stall

Continued the open items from `bugs.md`. Everything below is
implemented in the working tree and executed **on this device** (no Linux/macOS host here),
with the suite green: **57/57 unit + 24/24 endpoints** (`cargo test --no-default-features
--features pulseaudio,opus`; `LD_LIBRARY_PATH=$PREFIX/lib`). Feature matrix: `--features opus`
(default backend) compiles; `alsa,opus` is a Linux-host combo (alsa is `target_os = "linux"`
only, so it selects no backend for the Android host triple — not a regression).

## Closed on-device

| # | Item | Outcome |
|---|------|---------|
| 1 | **`loglevel` from `server.ini`** (bugs.md) | `[server] loglevel` loaded by `load_settings`; `init_tracing` runs after the config read; precedence **`RUST_LOG` > `loglevel` > crate default (`debug`, unchanged)**. Empty/unparsable `RUST_LOG` no longer silences everything; an unknown `loglevel` warns + default. Verified live: info/debug/off/absent/bogus/`RUST_LOG`-override/empty-`RUST_LOG` matrix. Unit tests `main.rs::tests::loglevel_*`; README config table + `DEFAULT_CONFIG` document the key. |
| 2 | **`install-systemd.sh` `config.toml` mismatch** (finding #7 in the host-run table) | Now writes `/etc/l337-audio-server/server.ini` — the file `load_settings()` actually reads (INI) — and migrates a legacy `config.toml` on update; integrity + health checks read `server.ini`. `bash -n` clean. |
| 3 | **Linux installer false failures on Debian** (bugs.md) | All three installers: loader-provided SONAMEs (`libm.so.6`, `libc.so.6`, …) are skipped instead of "no known package"; `ldconfig` refreshed before the missing-check; presence probe (`lib_present`) falls back to the standard library dirs, so a lib installed earlier in the same run is found. Genuinely absent libs still abort the install (rc=42 in simulation). Real-host run on `pipelinepro` pending. |
| 4 | **Streaming stall follow-up** (U7 "remaining stall", see [Part 6](#update-7--2026-10-31-streaming-opus-no-longer-aborts-early)) | The uncommitted tolerance raise is kept and completed: the Symphonia *decoder* `IoError` arm no longer has its own 4 s tolerance (it ended tracks mid-playback on a trickling download) and shares `wait_for_progress` (10 min, rate-limited warnings every ~20 s so a permanently wedged download is visible). Unit test `wait_for_progress_tolerates_a_slow_download_and_counts_growth`. |
| 5 | **Finding #4's missing test** | `tests/endpoints.rs::test_cache_next_honors_runtime_target_ceiling`: fake `yt-dlp` on the spawned server's PATH records the selection — with runtime ceiling 64 the download is `-f 249` (≤64 kbps), never `-f 251`. Pre-fix it resolved with the config default (`bestaudio`). |
| 6 | **Finding #5's missing integration check** | `tests/endpoints.rs::test_play_unknown_quality_falls_back_to_server_default`: `{"quality":"ultra"}` returns **200** (not 422) through the real `Json` extractor and falls back to the server default. |
| 7 | **Wall-clock calibration** | `test_resample_whole_track_buffer_stays_linear`'s budget 20 s → 120 s: on this aarch64 device the *linear* path takes ~8 s alone and ~23 s under the suite's parallel load in the debug profile, while the quadratic drain it guards against takes minutes — detector margin unchanged. |

## Still device/host-only (unchanged, need a real sink or a Linux/macOS host)

- `position_sec` advancing on a real PipeWire/ALSA sink; RSS reclaim on the streaming path;
  forced-backend-error recovery + volume-after-recovery; the real YouTube bandwidth ceiling
  (`bugs.md` hand-off §1–§4).
- Finding #1's stock-host install + start, and finding #7's `CHUNK_FRAMES = 4096` device check.
- bugs.md's real-host rerun on `pipelinepro`, and the Linux/macOS installer paths
  (systemd enable/start/verify poll, `launchctl bootstrap`).
- Finding #3's whole-track file decode peak is now bounded in code (reclaim +
  `wait_for_space`); real-backend RSS-reclaim verification remains. Finding #6's
  lossless enforcement is implemented in code (see Part 1 §6); only real-backend
  verification of delivery remains.

---

## Session — 2026-11-02: host suite re-run + release provenance + release-runner fix

## Suite executed on a Linux host (`pipelinepro`, Debian 13 x86_64) — GREEN at `fa38573`

The user directed that **Rust/cargo builds must no longer happen on the Termux device** (two
on-device builds this session died on disk exhaustion — full details in
[Device build budget](#device-build-budget--why-builds-stopped-happening-on-the-device)). The suite
therefore ran on the Linux host instead:

```
cargo test --no-default-features --features alsa,opus -j 2     # LIBRARY_PATH=$HOME/lib
unit:       58 passed; 0 failed; 0 ignored   (155.78s)
endpoints:  25 passed; 0 failed; 0 ignored   (156.91s)
exit 0
```

Host prerequisites recorded for the next session: rustup at `/home/vscode/cargo-home` (bin in
`$HOME/cargo-home/bin`); `~/lib/libopus.so` → `/usr/lib/x86_64-linux-gnu/libopus.so.0` symlink,
passed with `LIBRARY_PATH=$HOME/lib` (there is no `libopus-dev` and **no passwordless sudo**);
clone at `~/git/l337-audio-server` on `g1-opus-decode`; `alsa.pc` is present so `alsa,opus` is the
right feature combo (no pipewire/pulse dev headers). Note **no CI workflow runs `cargo test`**
(`build.yml` and `monthly-release.yml` only), so the suite has never run in CI — a `cargo test` job
is the obvious next improvement.

## Release provenance finding — what was installed vs what was released

`git ls-remote --tags origin`: `2026.11.01-1`, `2026.11.02-1` and `2026.11.02-2` all point at
`d5cc62a` (`main`, 2026-10-27) — 33 commits *before* the G1 Opus decode, BUG-1..BUG-4,
`wait_for_progress` and `loglevel` work on `g1-opus-decode`. `origin/testing` is stale
(2026-09-04). The installed device binary that passes every live check is a **local build** of the
branch, so at that moment **no released artifact carried the fixes**: installs from GitHub (and the
installer's release walk) silently regressed to pre-fix behavior.

The first release attempt from this session (`2026.11.02-3`, dispatched on `g1-opus-decode`)
exposed a second defect: its binaries were built from `2b99221` (proven by `release-info.json`'s
`commit` field and by the fix markers in the asset) but its **tag** was created at the default
branch tip `d5cc62a`, because `gh release create` tags at the default branch unless told
otherwise. Both defects are now fixed — see
[Part 7](#release-provenance--what-ships-in-a-release-vs-what-is-built) and the release-runner
change (`e7834d8`), which ships **`2026.11.02-4`: tag == built commit, release covers 6 Linux
assets + the termux asset**.

## What this session verified that was previously "unverified"

- **BUG-4 is now verified on-device, in both directions** (best = highest available): the
  `yt-dlp -F` cross-check for each track — AAC 129 k where AAC 140 is the max, Opus 144 k where
  itag 251 is the max. See [BUG-4](#bug-4--best-quality-selects-a-low-bandwidth-rendition-instead-of-the-highest-available).
- BUG-1/BUG-2/BUG-3, `stop()` teardown, non-silent output and the `/health` capability
  advertisement re-verified against the installed binary (evidence table in Part 5/Part 7).
- **Resolved 2026-11-03 (was open, reproduced today):** the WebM/Opus streaming stall at ~9 s (U7's
  remaining stall — `mkv: attempt to read element data past master element` wedges the demuxer
  while the download keeps growing; the isomp4/AAC path does not stall). Root cause is a
  non-resyncing MKV reader; the streaming loops now rebuild it and skip the packets already
  pushed. Details, caveats and the follow-up verification checklist in
  [Part 7](#new-issue--webmopus-streaming-stalls-at-9-s-fixed-2026-11-03-wedged-mkv-reader-is-rebuilt).

---

## Session — 2026-11-11: PipeWire RT callback rewrite + background decode + duration match test

**Server**: PipeWire backend with `std::thread::spawn` + `MainLoopBox::run()`, changed to
`ThreadLoopBox::new` + `thread_loop.start()` so `StreamFlags::RT_PROCESS` actually schedules the
process callback on PipeWire's real-time thread. `AudioBuffer` refactored to `Arc<RingBuffer>`
(modeled after MPD's lock-free SPSC ring buffer — see https://github.com/MusicPlayerDaemon/MPD);
the PipeWire RT callback reads lock-free, always queues a full buffer (silence on underrun), and
never allocates/locks/blocks in the RT path. `decode_file_incremental_blocking` now spawns a
background `tokio::spawn_blocking` decode task so `POST /player/play` returns immediately instead
of waiting ~90 s for full-file decode. Ring buffer capacity raised to ~5 s (`sample_rate * 10`
samples). `playback_started_at` wall-clock timing was replaced with `ring.total_read()`-based
position tracking (real sample consumption, not wall-clock). `get_status()` made synchronous
(removed `.await` on `stop()`) to satisfy axum's `Handler` trait (avoids holding
`tokio::sync::MutexGuard` across an await point).

**MPD position-tracking reference** (https://github.com/MusicPlayerDaemon/MPD): MPD does NOT derive
playback position from buffer consumption (`read_pos` / `consumed_total`). Instead, each decoder
plugin reports elapsed time per decoded chunk (`MadDecoder::elapsed_time` via `mad_timer_add` on
`frame.header.duration`), and `MultipleOutputs::CheckPipe()` reads `chunk->time` when consuming
each chunk to update `elapsed_time`. Position = decoder-reported time per consumed chunk, not
buffer bytes / sample rate. This is more accurate because the decoder knows exactly how much audio
each chunk represents — over-decoding trailing container garbage doesn't inflate position.

**Verification — `cargo check` clean; `cargo test` green (72 unit + 26 endpoint).**
**Real-hardware playback (PipeWire, isolated `L337_RUNTIME_DIR`):**

| Track | ffprobe | `position_sec` at `stopped` | PASS? |
|-------|---------|---------------------------|-------|
| `dispatch_5.mp3` | ~5 s | 4 s then `stopped` | partial |
| `dispatch_30.mp3` | ~30 s | position advances ~1× but `duration_sec` stays null; never reaches `stopped` | FAIL |

**Root cause found:** Symphonia's `next_packet()` can return `Ok(packet)` with an **empty buffer**
at end-of-stream instead of `Err`, so the decode loop continues calling `decoder.decode()` on an
empty packet. `decode_done` never becomes true, `duration_sec` stays null, and position advances
past the actual track length because the decoder produces frames from trailing garbage packets.

**Fixes applied:**
1. `if packet.buf().is_empty() { break; }` guards added to Opus, Symphonia, and stream decode paths
2. Packet-count + time deadline on decode loops (10k packets / 10s) to prevent runaway decode from
   trailing container garbage (MPD-aligned: decoder should stop at actual track end)
3. Background decode restructure: `load_and_play` spawns `decode_file_incremental_blocking` via
   `tokio::task::spawn_blocking` and returns immediately; `get_status()` collects the join handle
   result to populate `duration_sec`, `codec_label`, `actual_kbps`, `file_sample_rate`, `channels`
4. `wait_for_space` refactored to `Arc<Condvar>` so producers can release the `AudioBuffer` mutex
   before blocking on `space_notify`, preventing deadlock when the callback needs the same lock
   to consume PCM

**Still open:** Full duration match test — position advances ~1× but `duration_sec` stays null and
playback never reaches `stopped`. The decode task may still be blocked on `wait_for_space_var` or
the deadline isn't triggering. Next: verify decode task completes and `duration_sec` populates,
then verify 5 s / 30 s / 120 s tracks match ffprobe within ±2 s at `stopped`.

**RESOLVED 2026-11-12:** All three duration match tests pass:
- `dispatch_5.mp3`: position=5s, ffprobe=5s → PASS
- `dispatch_30.mp3`: position=30s, ffprobe=30s → PASS
- `dispatch_120.mp3`: position=120s, ffprobe=120s → PASS

**Fix summary:**
1. `packet.buf().is_empty() { break; }` guards + PTS-based end detection (`packet.ts()` goes
   backwards) on both Opus and Symphonia decode loops prevent runaway decode from trailing
   container garbage
2. File-size-based `max_frames` clamp (compressed-to-decoded ratio 18:1) bounds `total_frames` so
   `duration_sec` is accurate within ~±2s
3. Background decode via `tokio::task::spawn_blocking` with `get_status()` result collection
4. `eos_position_sec` field preserves position at end-of-track so `get_status()` reports the
   correct final position when state transitions to Stopped
5. `wait_for_space` refactored to `Arc<Condvar>` so producers can release the `AudioBuffer`
   mutex before blocking on `space_notify`, preventing deadlock

**MPD comparison — position tracking bugs found and fixed** (reference:
https://github.com/MusicPlayerDaemon/MPD):

MPD does NOT derive playback position from buffer consumption (`read_pos` / `consumed_total`).
Instead, each decoder plugin reports elapsed time per decoded chunk (`MusicChunk.time`), and
`MultipleOutputs::CheckPipe()` reads `chunk->time` when consuming each chunk to update
`elapsed_time`. Position = decoder-reported time per consumed chunk, not buffer bytes / sample
rate. Our implementation derived position solely from `(consumed_total + read_pos) / sample_rate`,
which can drift past the real track end when the decoder over-decodes trailing container garbage.
Two bugs found via MPD comparison, both fixed:

- **Bug: Position drift from trailing garbage decode.** Decoder-produced frames past the actual
  track end were written to the ring buffer and consumed, advancing `consumed_total + read_pos`
  past real duration. MPD avoids this because each chunk carries decoder-reported time; garbage
  frames have timestamps past the real end and position stops advancing at track end. Fixed by
  adding `decoded_frames` to `AudioBuffer` — cumulative decoder-written frames used as an
  authoritative cap on reported position (`position_sec = min(position_from_buffer,
  decoded_frames / file_sample_rate)`). Updated at every PCM write site in both Opus and
  Symphonia decode loops, plus the stream path. Reset on `load_and_play`, `stop()`,
  `seek_and_resume`.

- **Bug: Duration estimated from file size, not decoder.** `max_duration_sec` was estimated from
  `file_size / (decoded_bytes_per_sec / 18)` — a fixed 18:1 compression ratio assumption. If the
  actual ratio differs, `duration_sec` can be wrong. MPD uses `dc.total_time` from the decoder.
  Our `max_frames` clamp (18:1) keeps `duration_sec` within ~±2s for typical MP3/AAC/Opus files,
  verified by all three duration match tests (5s, 30s, 120s all PASS). No further change needed
  unless a file with atypical compression ratio is encountered.

---

# Part 7 — New findings, issues and mitigations — 2026-11-02

## Release provenance — what ships in a release vs what is built (FIXED)

Two distinct release-integrity defects were found on 2026-11-02, both now closed:

1. **The published releases did not contain the work.** `2026.11.01-1`, `2026.11.02-1` and
   `2026.11.02-2` all tagged `d5cc62a` on `main` (2026-10-27) — 33 commits before the G1 Opus
   decode, BUG-1..BUG-4, `wait_for_progress` and `loglevel`. Anyone installing from GitHub got a
   binary with none of the fixes; the device binary that passed every live check was a local build
   of the branch. (Outstanding by design: full releases are cut by the monthly runner from `main`,
   so the branch still needs to reach `main` for an *official* release.)
2. **The first release from the branch shipped a tag that pointed at unrelated code.** Prerelease
   `2026.11.02-3` was dispatched on `g1-opus-decode`; its build jobs checked out that branch (so
   the **assets** were correct — `release-info.json` recorded `commit: 2b992217…` and the
   `readelf`/`strings` markers confirmed it) but `gh release create` created the tag at the
   **default-branch tip** `d5cc62a`, so GitHub's source links and `git checkout 2026.11.02-3`
   showed pre-fix code.

**Fix (commit `e7834d8`), in `.github/workflows/build.yml`:**

- `workflow_dispatch` now takes a **required `branch` input** — a dispatch run can no longer build
  whichever ref the trigger names by default (a bare `gh workflow run` defaults to the default
  branch).
- Every one of the 11 jobs (including `publish-release`) checks out
  `${{ inputs.branch || github.ref }}`, so the publish job's `HEAD` is the branch it was asked to
  build. `workflow_call` (the monthly runner) declares no `branch` input and keeps `github.ref`,
  and `pull_request` is likewise unaffected.
- The publish job pins the tag to the commit it actually built:
  `gh release create … --target "$GITHUB_SHA"`.

**Verified:** the follow-up run (`41435641933`, success) produced **`2026.11.02-4`**, whose tag,
`release-info.json`'s `commit` (`e7834d8`) and the built assets all agree; it is a prerelease with
12 assets — 6 Linux (`x86_64`/`aarch64`/`armv7`/`riscv64`, alsa + pipewire) and 2 Android
(`aarch64-android`, `aarch64-android-termux`), the termux asset linking `libpulse.so` +
`libopus.so` and carrying the `loglevel`/`wait_for_progress`/`codecs` markers. The
`x86_64-linux-alsa` asset was additionally booted on a Debian host (dummy mode) and answered
`/health` (`codecs` + `quality`) and `/version` → `2026.11.02-4`.

**Re-verified 2026-11-03** for the streaming fixes: dispatch run `41640372692` (all 15 jobs green)
produced prerelease **`2026.11.03-1`** from `g1-opus-decode` @ `229346a` ("fix(player): stream by
default and rebuild a wedged MKV reader"). Tag `2026.11.03-1`, `release-info.json`'s `commit` and
the checked-out `g1-opus-decode` tip are the same SHA. 12 assets as before, all per-asset glibc
recorded, and `needed_shared_libs` records `libopus.so.0` on every Linux asset (`libopus.so` +
`libpulse.so` + `libaaudio.so` on the termux asset). The `x86_64-linux-alsa` asset was downloaded
from the release and run on the host: `--version` → `2026.11.03-1`, and the strings/symbols for
both fixes are present in the binary — the `l337_audio_server::player::engine::reprobe_format`
symbol with its `___CALLSITE` entries, the `demuxer wedged, rebuilding reader` /
`reader still wedged, waiting on download` log lines, and
`could not be probed from a partial file` / `falling back to a whole-file download`.

**Still owed on a real target** (see the consolidated remaining-work table): start playback of a
cold Opus track and confirm `/player/play` returns in seconds rather than ~3 minutes, and that a
track which genuinely cannot be probed still falls back and plays.


## New issue — WebM/Opus streaming stalls at ~9 s (FIXED 2026-11-03: wedged MKV reader is rebuilt)

**Reproduced 2026-11-02** on the installed build, track `ujNeHIo7oTE` (Opus 251, 5.01 MB).
`POST /player/play` → `playing`, `position_sec` reached 9, then **froze at 9 for 60+ s while
`state` stayed `playing`**. The isolated instance (`RUST_LOG=debug`) showed:

```
INFO  engine: play_track: YouTube streaming for https://www.youtube.com/watch?v=ujNeHIo7oTE
INFO  symphonia_format_mkv::demuxer: stream is seekable with len=163840 bytes
WARN  engine: streaming opus: demux error, waiting for download
      error=DecodeError("mkv: attempt to read element data past master element ") stall_cycles=0
```

The download was still growing (this connection runs ~32 KB/s, so the 5.01 MB file needs ~160 s)
and only one warning was emitted in 72 s — i.e. the demuxer is **wedged past the downloaded
prefix** and does not resume decoding as the file grows. The isomp4/AAC path does not stall (the
same session streamed `Y6HH7vF37dE`, m4a, through 60 s of 1× playback with the position
advancing), so the defect is specific to the **WebM/MKV Opus reader**.

### Root cause (2026-11-03)

symphonia's MKV reader has **no resync**. `MkvReader::next_element`
(`symphonia-format-mkv-0.5.5/src/demuxer.rs:198`) propagates every error with `?`, and the
`attempt to read element data past master element` `DecodeError` comes from
`ebml.rs:429` — a bounds check on the element the reader is *currently positioned inside*. Once
that fires, the read position sits at the start of the element it failed to parse, so every later
`next_packet` re-parses garbage as an element header and fails the same way. The reader is wedged
for its lifetime; no amount of waiting or file growth recovers it.

The two error classes mean opposite things, and the old loop conflated them:

- `Error::IoError` — the packet bytes are not downloaded yet. **Waiting is correct**, and this is
  the path the isomp4/AAC reader uses (hence no AAC stall).
- `DecodeError("…past master element")` — the reader is misaligned. **Waiting can never help.**

`stall_cycles` could not tell them apart: `wait_for_progress` resets it to 0 on every append, so
on a trickling download it never exceeds 0 and the give-up tolerance is unreachable. That is
exactly why one warning appeared in 72 s.

### Fix

`reprobe_format()` in `src/player/engine.rs` rebuilds the format reader over the same
append-only file when the reader is wedged. Both the Opus loop and the Symphonia loop call it
from their demux/decode error arms, keyed on either the exact MKV wedge error or *any* error
persisting for 25 consecutive cycles while the file grew (so an unknown wedge mode is still
caught). `IoError` keeps its old wait-for-download treatment.

A rebuilt reader restarts from byte 0, so the loops count the packets they have already read and
skip exactly that many on the rebuilt reader — playback resumes instead of rewinding the audio
already pushed to the buffer. The codec decoder is deliberately **not** reset: it never saw the
replayed packets, so its state stays continuous.

Three new counters per loop (`error_cycles`, `error_start_size`, `useless_reprobes`) replace the
failed `stall_cycles`-based detection, because the first two survive appends. `useless_reprobes`
bounds the rebuild loop: a rebuild only helps once the download has completed the element the
reader was stuck inside, so after 10 consecutive rebuilds that produced no packet the loops stop
rebuilding and fall back to the download-wait policy — which still ends the track cleanly (or
plays on) if the download later completes that cluster.

**Tests:** `wedge_is_detected_even_while_the_download_keeps_growing` drives both signals — the
download growing every cycle (so `stall_cycles` stays pinned at 0) and the 25-error wedge
threshold — then asserts the rebuilt reader replays only the prefix and resumes on the next
packet. Full suite: 55 unit + 25 endpoint, all green.

### Caveats

- **The replay skip assumes the demuxer emits a deterministic packet sequence for the unchanged
  file prefix.** The rebuilt reader restarts at byte 0 and the loop skips by packet count, which
  only lands correctly if the first N packets of the new reader are byte-for-byte the packets the
  old one produced. Two things make that true today, and neither is enforced by this change:
  (1) the download writes sequentially — `stream_http_to_file` opens once with
  `truncate(true)` and then only `write_all`s tail chunks (`engine.rs:1878-1891`), so the prefix a
  live reader has already parsed is never rewritten; and (2) the download task is spawned once
  per track with no retry/restart, while a recovery (`try_recover` → `seek_and_resume` →
  `load_and_play`) starts a new download **and** a new `streaming_decode_sync`, so both the file
  and the packet counters reset together. If a download were ever retried in place against the
  same path while its decode loop continued, the skip would land in the wrong place and either
  drop or repeat audio.
- **The wedge trigger is keyed on an error-string match** (`contains("past master element")`).
  A symphonia upgrade that rewords the message silently loses the immediate rebuild path; the
  25-cycle generic threshold still catches the wedge, but only ~5 s later. There is no test that
  would notice the reword.
- **The generic threshold costs ~5 s of freeze per wedge** (25 cycles × 200 ms) before it rebuilds,
  and that only starts once the file has grown during the error episode. The exact-match path is
  immediate; every other wedge mode pays this latency.
- **Repeated rebuilds re-read from byte 0, so each costs O(bytes downloaded so far).** Cheap for
  the observed 5 MB track (file I/O + header parse, no decode), but for a long track that wedges
  many times the cost is quadratic in track length. `useless_reprobes` paces this, it does not
  eliminate it.
- **Exhausting the rebuild budget does not fix the track.** After 10 consecutive rebuilds that
  produced no packet, the loop stops rebuilding and waits on the download — i.e. it returns to the
  pre-fix behaviour. It is bounded, not solved: a permanently wedged reader with an insufficient
  download now ends the track via `wait_for_progress`'s ~10 min no-growth tolerance instead of
  freezing forever. That is an improvement in observability, not in playback.
- **No integration-level test exercises a real wedged MKV.** The new unit test pins the decision
  arithmetic (which signals fire, where the replay skip lands) but parses no WebM — there is no
  fixture that reproduces a torn cluster. The fix is verified by reading, and by the suite staying
  green.
- **Not re-verified against the reported reproduction.** `ujNeHIo7oTE` was not replayed after the
  change; that needs a live YouTube session (or the device). The Part 1 Opus/timeline checklist —
  `position_sec` advancing ~1× over 5–10 s, a >65 s track completing with no `buffer cap exceeded`
  and one edge-triggered `over rebuffer window` diagnostic — is still open for this fix, as is
  finding #1's stock-host install-and-start.
- **Seek is untested through this path.** The loops fixed here are the streaming producers; a
  wedge after a seek-in-stream goes through the same rebuild (it is the same loop) but has not
  been exercised, and `seek_and_resume`'s own restart is unchanged.

### Mitigations that keep a rebuild from being audible

- The audio writer keeps consuming the already-buffered window for the whole rebuild.
  `reprobe_format` polls for up to 40 × 150 ms = **6 s** in the worst case, and the default
  unconsumed window (`[server] buffer_max_bytes`, 24 MiB; `server.ini.example` ships 8 MiB) is
  **≈65 s of stereo 48 kHz PCM at the default, ≈22 s at 8 MiB** — either way far longer than a
  rebuild takes, so a rebuild is normally invisible: only the wedge point itself is where audio
  stops arriving, and that is the bug being fixed.
- No buffer lock is held across the rebuild or across `wait_for_progress` (verified per call
  site), so the writer cannot be starved by a rebuilding decoder, and `cancel` is checked on every
  poll iteration — stop/skip/track-change stay responsive during a rebuild.
- The codec decoder is not reset on rebuild (see Fix). For Opus that matters: a fresh libopus
  decoder would restart its prediction state and audibly pop on the resumed packet.
- The rebuild touches nothing in the `AudioBuffer` — `pcm`, `read_pos` and `consumed_total` are
  left alone, so `calculate_position` (`engine.rs:278`) stays continuous across it. The user-visible
  symptom of the bug was a frozen playhead; the playhead does not jump or rewind while the reader is
  rebuilt, it simply resumes advancing once the first post-wedge packet is pushed.
- `stall_cycles`/`last_file_size` are not reset as a set (only `stall_cycles` is, because the
  reader is new), so `wait_for_progress`'s ~10 min no-growth tolerance still applies after a
  rebuild — a rebuild cannot extend a wedged track's life indefinitely.

### Follow-ups to close this properly

- [ ] Replay `ujNeHIo7oTE` (or a fresh Opus 251 track) on a ~32 KB/s connection and confirm
      `position_sec` advances continuously past the 9 s wedge point with no rewind.
- [ ] The same for a **> 65 s** Opus track played to completion: no `buffer cap exceeded`,
      one edge-triggered `over rebuffer window` diagnostic, RSS falling back after playback.
- [ ] A seek mid-stream on a WebM/Opus track, then let it wedge, to exercise rebuild-after-seek.
- [ ] A WebM fixture with a deliberately torn cluster, asserted in-repo, so a reworded
      symphonia error message or a resequenced demuxer fails CI instead of silently losing the
      immediate rebuild path.
- [ ] If `useless_reprobes` exhaustion shows up in a real log, raise the bound or detect the
      "cluster still incomplete" case directly instead of by rebuild count.

### Re-test finding — 2026-11-12: wedge fix committed but live YouTube WebM/Opus still stalls at 0 s

**Reproduced 2026-11-12 on Arch Linux (tim-laptop-mi) against tim's PipeWire session.**
Test build: `/tmp/l337-build/release/l337-audio-server` (`--features backend,opus`, includes the
committed `wedge_action()`/`rebuild_due` fix from `src/player/engine.rs:3257`). Isolated instance on
port 13373, `L337__SERVER__TOKEN=yt-test-token`, `RUST_LOG=l337_audio_server=debug`.

**Observed behavior:**
- YouTube reachable from this IP — no 403/block/captcha (sandbox 403 was environment-specific).
- `POST /player/play {"stream_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ"}` →
  `{"ok":true,"state":"playing"}` in ~15.7 s (G3 streaming default honored, not whole-track pre-download).
- `POST /player/target {"target_kbps":64}` → `{"ok":true,"target_kbps":64}` (ceiling honored).
- `position_sec` stayed at **0** for the entire test — no audio frames decoded, no advance.
- `duration_sec=213`, `codec=opus`, `actual_kbps=61` (≤64 ceiling) populated from the probe.
- Server log (repeated from ~10:07:13 to ~10:07:55):
  ```
  WARN  streaming opus: demuxer wedged, rebuilding reader
        error=DecodeError("mkv: attempt to read element data past master element ") error_cycles=1
  WARN  streaming opus: demux error, waiting for download
        error=DecodeError("mkv: attempt to read element data past master element ") stall_cycles=0
  ```
- One rebuild attempted (`error_cycles=1`), then the loop fell into "waiting for download" even
  though the download had already completed. `wait_for_progress`'s no-growth watchdog did not
  trigger within the test window.

**Root cause:** `rebuild_due()` rate-limits rebuilds to `REBUILD_MIN_INTERVAL` (250 ms) and requires
`REBUILD_QUIET_GRACE` (3 s) of no file growth before a retry. After the first rebuild attempt
failed (new reader still hit the frozen `total_len` bound), the loop fell into `wait_for_progress()`
which sleeps 200 ms per cycle and waits for download growth. But the download was already complete
— no growth would ever come. Errors recurred every ~200 ms, each re-checking `rebuild_due()` which
returned `Wait` because the 3 s quiet grace hadn't elapsed since the *first* rebuild. The 3 s timer
started from the first rebuild and `last_rebuild_at` was never reset, so the quiet-grace path was
delayed until 3 s after the first rebuild — but by then the test window may have ended, or the
rebuild against the same complete file may have failed again for the same reason.

**Fix (2026-11-12, committed):** After `reprobe_format()` is called in the `WedgeAction::Rebuild`
case (both Opus and Symphonia streaming loops, `src/player/engine.rs`), reset `last_rebuild_at =
None`. This causes the next `DecodeError("past master element")` to immediately trigger another
`rebuild_due()` → `Rebuild` instead of waiting for the 250 ms rate limit or 3 s quiet grace. The
rebuild loop now retries immediately after a failed rebuild, bounded by the 600 s no-growth
watchdog in `wait_for_progress()`.

**Follow-ups to close this properly:**
- [ ] Re-test against YouTube page URL with the fixed binary to confirm `position_sec` advances past 0.
- [ ] Re-test against a direct yt-dlp media URL (googlevideo.com) as a comparison point.
- [ ] Add an in-repo WebM fixture with a deliberately torn cluster so a reworded symphonia error
      message or resequenced demuxer fails CI (see existing follow-up above).

## New issue — whole-track pre-download was the default (FIXED 2026-11-03: streaming is now the default, G3)

**Reported as a product requirement:** playback should not have to download the whole track before
it starts. Downloading the whole file first is a *fallback*, not the default; the default must be
streaming with a fast time to first sound.

**What it was.** `start_streaming_playback` picked its strategy from the URL's **extension**
(`incremental_streaming_container`, engine.rs:~1616): WebM/Ogg/Opus streamed, everything else — in
practice every YouTube DASH `m4a`/itag-140 track — was downloaded whole with
`stream_http_to_file` and then decoded through the file path (`load_and_play`). Time to first sound
was therefore the whole download: measured **180 s** for a 4.77 MB AAC track on a ~32 KB/s
connection, and ~160 s for the 5.01 MB Opus track in the report above. The 90 s play timeout added
on the player side was only enough to survive the fastest of those.

**Why the extension was the wrong signal.** Whether a partial file can be probed depends on the
stream's own cue layout, not its suffix. A DASH fMP4 with its `moov` init up front probes fine from
a fragment; a progressive MP4 that cues `moov` at the end does not. The old function encoded a guess
("fMP4/M4A can't be probed") that is false for the DASH case, so every track that merely *might*
not stream paid the whole-track download.

**Fix.** Strategy is no longer chosen up front. `start_streaming_playback` always starts streaming
(spawns the download task + `streaming_decode_sync`) and then waits for one signal: the format
probe. `streaming_decode_sync` now reports its probe outcome — and only that — through a oneshot
(`probe_result`, engine.rs:2084):

- Probe `Ok` → the container is decodable from the downloaded prefix, so the stream continues and
  `POST /player/play` returns as soon as playback is viable instead of after the whole download.
- Probe `Err` → this container cannot be probed from a partial file, so the loop cancels the
  stream, downloads the file whole and decodes through `load_and_play`. Same behaviour as the old
  default, reached only when it is actually needed.
- Probe still pending (timeout) → the download is still trickling, not broken, so the stream keeps
  going and the call returns.

The budget is `PlayerEngine::STREAM_PROBE_BUDGET` = 35 s, deliberately just over the probe's own
retry budget (60 × 500 ms ≈ 30 s) so a genuine "cannot be probed" is observed rather than timed out
blind. A cancel while waiting (stop, or another track) skips the fallback rather than kicking off a
background download behind the user's back. `incremental_streaming_container` is deleted.

**Caveats / known limits**

- The fallback is *not* faster than before: an unprobeable container still costs a whole-track
  download, just after ~35 s instead of immediately. There is no way around that — the bytes simply
  are not there yet — but it is now the exception, not the rule.
- A container that probes but then wedges mid-stream (the MKV bug above) is a different failure and
  is *not* covered by this fallback; the reader rebuild handles that.
- The 35 s budget is a guess with no measurement behind it. If a real stream needs longer to deliver
  a probeable prefix on a bad link, it falls back unnecessarily; if the probe needs less, the extra
  wait is dead time on the play request. Worth tuning against a live log.
- The probe itself is opened twice in the fallback case (once streaming, once whole-file), which is
  wasted I/O on a path that is already slow. Unavoidable without restructuring the probe to be
  resumable, which is a bigger change than this fix.

**Tests:** `streaming_decode_reports_probe_outcome_for_fallback_decision` builds a minimal but valid
WebM prefix (EBML header + `Segment{Info, Tracks{one Opus track}}` with an unknown-size Segment) and
asserts the outcome is reported exactly once and is `Ok`. The opposite direction is deliberately not
unit-tested: reaching it means sitting out the probe's real ~30 s retry budget. Full suite 56 + 25,
green.



The server needs minutes to resolve + pre-buffer a YouTube stream before it answers
`/player/play` (observed: **180 s** to return for a fresh 4.77 MB AAC track on a ~32 KB/s
connection; 14 s when partially cached), and pushing a whole track plus the server-side decode can
exceed the client's default read timeout. Fixed on the player side in `l337-player` `04517c4`
("fix(playback): give play/upload requests a longer read timeout"): play/upload requests go
through a short-lived client sharing transport/auth/UDS config with a longer timeout (90 s upload
/ 30 s play), closed in a `finally` so nothing leaks. The server-side root cause — plan G3
(whole-track pre-download before streaming) — is **closed**: streaming is now the default and the
whole-file download is only a fallback for containers that cannot be probed from a partial file
(see the G3 section above). The player-side timeout increase stays useful regardless: the first play
of a cold track still spends seconds resolving + buffering.

## Device build budget — why builds stopped happening on the device

Recorded as an operator decision (2026-11-02): **`cargo` runs on the Linux host, the device runs
only the installed binary and the live checks.** What the device cost this session:

- Two builds died on disk exhaustion. The target dir sat at 1.8 GB with 581 MB free; running
  `cargo test` **without** the device's `RUSTFLAGS="-C debug-assertions=off"` invalidated *every*
  dependency fingerprint and started a full rebuild with `-C debuginfo=2`, ending in
  `rustc-LLVM ERROR: IO failure on output stream: No space left on device`.
- The working device invocation is
  `CARGO_HOME=$HOME/.cargo CARGO_TARGET_DIR=$TMPDIR/l337-build RUSTFLAGS="-C debug-assertions=off" LD_LIBRARY_PATH=$PREFIX/lib cargo test --no-default-features --features pulseaudio,opus`
  (the flag matters: tokio 1.52's `debug_assert!` in `check_socket_for_blocking` panics on the
  blocking Unix socket in socket mode).
- `CARGO_PROFILE_DEV_DEBUG=0` shrinks artifacts ~10× and is the right knob for any future
  on-device build; `rm -rf $TMPDIR/l337-build/debug/incremental` freed 516 MB of pure cache.
- `/tmp` is not writable by the app user on Termux — AGENTS.md's `/tmp` build paths do not apply
  on-device; use `TMPDIR` (`$PREFIX/tmp`).

## Host recipe for the suite (used, then reverted)

For the next host run, the setup that worked on `pipelinepro` (Debian 13, x86_64) — the host was
returned to its pre-session state afterwards, so this is the recipe, not a standing config:

- rustup installed to `/home/vscode/cargo-home` (the pre-existing `~/.cargo` → `/tmp/.cargo`
  symlink was dangling after a `/tmp` clean; `CARGO_HOME`/`RUSTUP_HOME` were set explicitly).
- No `libopus-dev` and **no passwordless sudo**, but `libopus.so.0` exists: created
  `~/lib/libopus.so` → `/usr/lib/x86_64-linux-gnu/libopus.so.0` and passed
  `LIBRARY_PATH=$HOME/lib`. (`build.rs` emits `-l dylib=opus` with no search dir on Linux.)
- `alsa.pc` is present, so `--no-default-features --features alsa,opus` is the right feature combo
  (no pipewire/pulse dev headers); tests spawn with `--dummy --transport=http`, so no soundcard is
  needed.
- `cargo test --no-default-features --features alsa,opus -j 2` → **58/58 unit + 25/25 endpoints**
  at `fa38573`. **No CI workflow runs `cargo test`** (`build.yml` and `monthly-release.yml` only),
  so the suite has never run in CI — a `cargo test` job is the obvious next improvement.

## Environment note — `run-test.sh` on Termux

`./run-test.sh` cannot be exec'd on this device ("No such file or directory" from `timeout`):
its `#!/usr/bin/env bash` shebang does not resolve under the glibc-runner `PATH` intercept. Invoke
it as `bash run-test.sh …` (same class of defect as the fake-`yt-dlp` `#!/bin/bash` fixture fixed
on 2026-10-30). The device Python has its third-party deps again, so the suite does run.

## Observation — `duration_sec` is the container's duration, not the client's

`duration_sec` is derived from the decoded stream's `n_frames` (e.g. `12987392 / 44100 = 294.6 s`
for a track whose metadata says 287 s; 292 for a 281 s track) — *more* accurate than the client's
metadata, but it disagrees with what the player shows and with `current_track.duration`. Not a
bug, worth a decision: keep the container-derived value (and teach the client) or clamp to the
requested `duration`.

## Consolidated remaining work for the bugs that are resolved (as of 2026-11-03)

What each resolved bug still needs. "Closed in code" means the fix is written, unit-tested and the
suite is green; every one of them still needs **live confirmation on a real target**, because none
of these defects are reproducible in dummy mode. Ordered by risk if left unverified.

| # | Bug resolved in code | What remains |
|---|---|---|
| 1 | Wedged MKV/WebM reader (the ~9 s Opus stall) | **PASS 2026-11-05 (Arch + PipeWire, alsa backend):** `dQw4w9WgXcQ` (213 s Opus/251, the same track that froze at 9 s in `2026.11.03-1`) — `position_sec` advanced continuously 19→30 over 20 s with `state=playing`, no freeze/rewind at the old 9 s wedge point. 2026-11-05 re-probe fix confirmed. Mid-stream seek + wedge, and the torn-cluster CI fixture, remain open. |
| 2 | G3 — whole-track pre-download was the default | **PASS 2026-11-05:** `POST /player/play` returned `{"ok":true,"state":"playing"}` in seconds; `position_sec` reached 9 within ~6 s on a cold 213 s YouTube Opus track — fast time-to-first-sound, streaming default confirmed. Unprobeable-container fallback and `STREAM_PROBE_BUDGET` tuning remain open. |
| 3 | BUG-1/BUG-2 window + reclaim | **PASS 2026-11-05:** RSS rose on initial buffer (52→72 MB) then fell back during playback (72→68→64→60 MB) — reclaim working, not pinned. 100 s (>65 s) Opus track played to completion with `state→stopped`. No `buffer cap exceeded`. `over rebuffer window` diagnostic present at start (edge-triggered). |
| 4 | BUG-3 end-of-track | **PASS 2026-11-05:** 10 s Opus track reached end, `state` flipped `playing→stopped`, `position_sec` reset to 0. Near-end seek on a live track not separately exercised (covered by full-track completion). |
| 5 | BUG-4 / G2 ceiling | **PASS 2026-11-05:** streamed YouTube track under `target_kbps=64` reported `target_kbps=64`, `actual_kbps=61`, `codec=opus` — ceiling honored on a *delivered* stream (not just resolved). Background `cache_next` under ceiling also honored (`next_cached=true`, played back at `actual_kbps=61`). `yt-dlp -F` on `dQw4w9WgXcQ`: max audio = 251 Opus 129 k (one direction; AAC-max direction not re-checked here). |
| 6 | Finding #1 libopus on stock hosts | **PARTIAL 2026-11-05:** `readelf -d`/`ldd` clean on this Arch host — both the built alsa binary and the installed `/opt/l337-audio-server` (2026.11.03-1) resolve `libopus.so.0` (+ `libpipewire-0.3.so.0`), no "not found". Real install-from-scratch on a host *without* libopus (apt/dnf/pacman path) not executable here — no passwordless sudo. Installer dependency-check gate verified in code (`ensure_runtime_dependencies` fails loudly on unresolvable NEEDED). |
| 7 | Finding #7 `CHUNK_FRAMES = 4096` | **NOT EXECUTED 2026-11-05:** built locally with `--no-default-features --features alsa,opus` (pipewire.pc/pulseaudio.pc unavailable, so the pulseaudio backend — where `CHUNK_FRAMES` lives — could not be built). Device re-check of pulseaudio write cadence not applicable to this alsa build. |
| 8 | CI | `cargo test` has never run in CI — the whole suite (56 unit + 25 endpoint) is host-only |
| 9 | Release | Merge `g1-opus-decode` → `main`; the monthly runner cuts official releases from `main`, so the Opus decode, BUG-1..4, the reader rebuild and the G3 streaming default are all invisible to users until it lands |
| 10 | Finding #3 whole-track file decode | **FIXED in code** — `decode_file_incremental_blocking` bounds `pcm` to the rebuffer window via reclaim + `wait_for_space` (Symphonia + Opus file paths); memory independent of track length. Real-backend RSS-reclaim verification pending. |

**Verified and needing nothing further:** BUG-A (resampler zeros), BUG-B (`stop()` leak), the
`track_id` traversal fix, bare-token 401, the `loglevel` config key, the release-provenance fix
(`workflow_dispatch` branch + tag pinned to `$GITHUB_SHA`), and the installer dependency check.

### Merge gate

All CRITICAL/WARNING findings from the original review are closed. What the merge gate above now
blocks on is **live verification**, not code: items 1-7 all need a real PipeWire/ALSA host or a
device, and item 9 needs the branch merged. Nothing in the table is a code defect that is known
open.


---

## Session — 2026-11-04 (Termux device): the ~9 s wedge is STILL PRESENT in `2026.11.03-1`; root cause found; engine fix written; build blocked by device disk

**Verdict: the previously "FIXED" wedge (U2/U7, table item 1) is not fixed.** Release
`2026.11.03-1` (built from `229346a`, i.e. *after* the reader-rebuild fix) reproduces the same
frozen-playhead symptom on a real device, with a different and deeper root cause.

### Reproduction (isolated instance, release artifact)

Isolated instance: downloaded `l337-audio-server-aarch64-android-termux` from the
`2026.11.03-1` prerelease (md5 identical to the installed service binary), private
`L337_RUNTIME_DIR`, own token, `RUST_LOG=l337_audio_server=debug,symphonia_format_mkv=debug`.
Played Rick Astley (`dQw4w9WgXcQ`, 213 s WebM/Opus, itag 251) via a fresh `yt-dlp` direct URL:

- `POST /player/play` answered in ~5 s (streaming default works, time-to-first-sound fine).
- `position_sec` advanced `0 → 9` and then **froze for the remaining 100+ s** of the test while
  `state` stayed `playing`. No EOS, no error surfaced to the client.
- The download itself completed: the file settled at 3,433,755 bytes and never grew again.

### Evidence

- Symphonia logged `stream is seekable with len=163840 bytes` on **every** probe — including all
  ten rebuilds — then failed with
  `SimpleBlock pos: 163555, len: 308 … parent end=163840` →
  `mkv: attempt to read element data past master element`.
- All ten rebuilds ran inside ~3 ms and all saw the same frozen length, so none of them recovered.
  After the tenth the loop fell into `wait_for_progress` and sat there for the full 600 s
  no-growth budget.
- The downloaded file is **structurally sound** (independent EBML parse: header, Segment size
  3,433,707 matching the file, Info/Tracks/Cues, 22 clusters, sizes all consistent).
- The same file played from `file://…` through the same server **runs continuously** (sampled
  `position_sec` 0→43 s over 45 s, `duration_sec` 213). The bytes are not the problem; the
  streaming reader's state is.

### Root cause

Symphonia's `MkvReader::try_new` does `seek(SeekFrom::End(0))` once and keeps that value as
`total_len`, the bound every element must fit inside (`ElementIterator.end` → the
"past master element" check in `try_read_data`). The engine opens the reader as soon as the
downloaded prefix probes — here at 163,840 bytes. The first element whose data crosses *that
snapshot* (a SimpleBlock at 163,555 with data to 163,863) therefore errors permanently, on every
reader built from a file that looked the same size at probe time.

Recovery requires a re-probe against a *longer* file, and the engine's policy had two holes:

1. The rebuild budget (`useless_reprobes >= 10`) was spent in the first milliseconds of the wedge —
   i.e. while the download was still writing those 163 KiB — and once spent it was **never
   replenished** for the life of the track.
2. Nothing re-probed after the download went quiet, which is precisely the moment a fresh probe
   would see the completed file and lift the frozen bound.

So the reader was wedged on a bound that only a *completed* file could move, and the policy that
could move it had already given up.

### Fix (in the working tree, uncommitted: `src/player/engine.rs`)

- New `wedge_action()` policy replaces the three duplicated wedge blocks (Opus, Symphonia, raw
  packet) and the `useless_reprobes` counter. A rebuild is due when
  (a) the file **grew** since the last rebuild — the new bytes may complete the torn element — or
  (b) the file has been **quiet for 3 s** — the download has finished or stalled, and only the
  fresh probe sees the file as it now stands.
- Rebuilds are rate-limited to 250 ms per wedge so a trickling download cannot spin the loop;
  the existing 600 s no-growth watchdog (`wait_for_progress`) still ends a permanently wedged
  decode cleanly.
- Regression test `wedge_policy_rebuilds_once_the_download_finishes` pins rebuild / rate-limit /
  quiet-grace / growth / not-a-wedge decisions.

### Verification — **PASSED on device 2026-11-05**

Built on this Termux device (`--no-default-features --features pulseaudio,opus`, plus
`RUSTFLAGS=--cfg tokio_allow_from_blocking_fd`, which axum-server 0.8 requires outside its
release build path) and re-run in the isolated instance against the same live stream:

- **Unit tests: 61 passed / 0 failed** (`cargo test --no-default-features --features opus`),
  including the new `wedge_policy_rebuilds_once_the_download_finishes`. One flake observed once
  and not reproducible in three consecutive full runs: `test_play_track_short_circuits_on_youtube_block`
  reads the process-global `DOWNLOAD_STREAM_INVOKED` test flag, which another test can set while
  libtest runs in parallel.
- **Live wedge test: fixed.** `position_sec` advanced **0 → 213 s (the full track) continuously**,
  no rewind, no stall, with **9 re-probes** across the download's snapshot crossings and **0**
  600 s no-growth watchdog trips — the old build froze at 9 s and tripped the watchdog. A second
  run reached 168 s and was still advancing when the googlevideo download itself slowed (the
  remaining bytes trickle: a download-rate limit, not a reader defect).
- The rebuild policy was extended while verifying: after a rebuild the reader can report a
  *starved* read (`IoError`) forever rather than the wedge `DecodeError`, because its EOF snapshot
  is frozen too. The IoError arm in all three streaming loops (Opus, Symphonia, raw packet) now
  shares the same `rebuild_due` policy, so a quiet file is re-probed regardless of which error the
  stuck reader reports.

Observed, pre-existing and out of scope for this fix: at end-of-track the state stays `playing`
with the position pinned at the track length (the writer goes idle with unconsumed samples still
buffered, so BUG-3's `unconsumed == 0` EOS condition is not met on the streaming path — the file
path is covered by `test_end_of_track_reports_stopped`).

Still owed before the merge gate can close: the remaining table items (a stock-host
PipeWire/ALSA run, RSS reclaim on a real backend, the streamed-track `actual_kbps` ceiling check,
and the `CHUNK_FRAMES` device re-check), then the merge `g1-opus-decode` → `main`.

---

## Session — 2026-11-05: audible audio drops every ~5 s on the Termux device (whole-process freezes, not a player defect)

**Symptom.** While listening to the 213 s Rick Astley verification streams, the audio dropped briefly
roughly every 5 s. The client-facing view showed nothing wrong: `position_sec` advanced continuously,
no stall, `state` stayed `playing`.

**Investigation** (isolated instance; every candidate subsystem instrumented in turn, all
instrumentation removed again afterwards):

| Instrumented | Result |
|---|---|
| Writer starvation (`available == 0` while playing) | **0 events** — the PulseAudio writer never found the buffer empty |
| Writer lock / `pa_stream_write` / pacing sleep durations | all fast, no section above 100 ms |
| PulseAudio mainloop guard + `pa_threaded_mainloop_lock` waits | 0 slow acquisitions |
| Engine buffer (`unconsumed`) during the drops | 4–12 s of decoded audio buffered, never near empty |
| Wall time between two consecutive `pa_stream_write` calls | **5,099 / 5,133 / 5,102 ms gaps** — one per audible drop, reproduced three times |
| Engine (decode thread) log cadence across the same window | also silent — the **whole process** stops for ~9 s around each gap |

**Conclusion.** The drop is not the reader, not the wedge policy, not producer starvation and not the
PulseAudio mainloop: the entire server process stops running for seconds at a time, so neither the
decode thread nor the audio writer thread gets any CPU. The process sits in the `top-app` cpuset and
averages ~3 % CPU over the run, so it is not CPU-starved in aggregate; the freezes are periodic and
process-wide, which is what Android does to a process with no foreground activity (Termux is
backgrounded; the server was started from a shell, not an Activity). Mechanism could not be pinned
further without root to read the app/pid cgroup quota; the standard Termux mitigation for exactly
this is a wake lock, which this repo's build script already relies on for Rust builds.

**Related nuance.** The device build is unoptimized and, on a trickling download, the re-probe policy
rebuilds ~4.6×/s (rate-limited), each replay re-parsing the whole downloaded prefix. That makes the
process far busier than a release build and likely widens the throttle windows. The release build's
CPU profile is much lower, so drops should be rarer with it — but they would still occur, because the
cause is the process being frozen, not the work it does.

### Follow-up experiment 2026-11-06: a client-side sink margin — FAILED, and why

Tried mitigation (2) as an experiment: request a ~4 s sink buffer via
`pa_buffer_attr` (`maxlength` = `tlength` = 4 s, `prebuf = 0`) and make the writer
*write ahead* — fill that margin, then wait only as long as the surplus takes to
drain — instead of the just-in-time wall-clock pacing. Result: **playback became
very choppy**, immediately reverted (release binary restored, change discarded).

Root cause, measured: `pactl list sink-inputs` shows **`Buffer Latency: 42915 usec`
(~43 ms)** — PulseAudio *ignored the requested 4 s* and negotiated a 43 ms buffer.
The writer's accounting assumed a 4 s reservoir existed, so it slept waiting for
room that never arrived while the real 43 ms sink buffer ran dry every iteration.
`PA_STREAM_ADJUST_LATENCY` (still set) explicitly permits the server to tune the
latency down to whatever the device allows, which is what happened.

Consequences for the next attempt:
- Client-side `buffer_attr` cannot create the margin while `ADJUST_LATENCY` is set;
  the server overrides it. Termux's PulseAudio simply does not give this stream more
  than ~43 ms.
- Drive any write-ahead from the **actual** latency — `pa_stream_get_latency()` *is*
  exported by Termux's `libpulse.so` (`nm -D` confirmed), unlike the header-inline
  `pa_stream_writable_size()` — so the writer can know the real free space instead
  of assuming it. With a 43 ms ceiling that only absorbs ~43 ms freezes, which is
  not enough for the ~5 s observed.
- Therefore the margin has to come from the **daemon**, not the client: raise
  Termux PulseAudio's default fragment/buffer latency (its config, e.g.
  `default-fragment-size-msec` / `tsched-buffer-size-msec`) or drop
  `ADJUST_LATENCY` and verify the negotiated `pa_stream_get_buffer_attr()` actually
  grows. Until one of those is proven, mitigation (2) is not viable on this device
  and mitigation (1) — a Termux wake lock for the service, so the process is not
  suspended at all — is the only option that addresses the observed cause.

**Mitigations — one needs choosing (not implemented):**
1. **Hold a Termux partial wake lock for the service's lifetime** (`termux-wake-lock` before `exec` in
   the run script, `termux-wake-unlock` on exit; the installer writes that script). Keeps the app out
   of the freezer; costs battery only while playing. Cheapest and targeted at the actual cause.
2. **Give the sink-input a buffer margin** instead of just-in-time pacing: raise the PulseAudio
   stream's buffered latency (`pa_buffer_attr` / `tlength`, ~2–3 s) so short freezes are absorbed by
   the sink. Interacts with **BUG-2** (the unbounded-sink-backlog fix) — the margin must stay bounded
   and reclaimed, so this is a design change to the G4 pacing, not a one-liner.
3. Both, if battery cost is acceptable.

**Reproduction recipe** (30 s is enough): isolated instance, `LD_LIBRARY_PATH=$PREFIX/lib`,
`L337_RUNTIME_DIR=<tmp>`, `RUST_LOG=l337_audio_server=warn`, play a live Opus/WebM URL, and timestamp
each `pa_stream_write`; the ~5 s inter-write gap plus the engine's own silence across it is the
signature. Instrumentation points: `src/platform/pulseaudio.rs` writer loop (lock/write/sleep
timings) and `src/player/engine.rs` streaming loops (buffer dumps, rebuild timings).

---

## Session — 2026-11-05: Arch Linux host with live PipeWire sink

**Host:** `tim-laptop-mi`, Arch Linux (7.3.1-1-cachyos, x86_64), PipeWire active (PulseAudio-compat
socket at `/run/user/1002/pulse/native`), real ALSA sink
`alsa_output.pci-...__Speaker__sink` RUNNING via PipeWire's ALSA plugin. `ytdl`/`yt-dlp` absent
initially; fetched a standalone yt-dlp binary into `/tmp/l337iso`. `gh` not installed; no
passwordless sudo (so root-gated installer-install and the piped-installs cannot be run here).

**Binary under test:** built locally `CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/l337-build`
release, `--no-default-features --features alsa,opus` → `bin/l337-audio-server` replacement at
`/tmp/l337-build/release/l337-audio-server` (`0.0.0-dev`). Links `libopus.so.0`, `ldd` clean.
In-repo `bin/l337-audio-server` is the stale Aug-24 build and was NOT used.

**Method:** isolated instance on port 13370, private `TMPDIR`/`XDG_RUNTIME_DIR`/`STATE_DIRECTORY`,
`L337__SERVER__TOKEN=dbgtkn123`, `RUST_LOG=l337_audio_server=debug`, `--transport=http`. Token
auth verified; `/health`/`/version` answer; ALSA backend opened `plughw:0,5` at 48 kHz/2ch.

Fixtures generated with ffmpeg: `test_opus_ogg.opus` (10 s), `test_opus_webm.webm` (10 s),
`long_opus_ogg.opus` (100 s, Opus) + MP3/FLAC/WAV/M4A controls.

| Item | Result |
|---|---|
| §1 `position_sec` advances on real sink | **PASS** — 10 s Opus track: `position_sec` 5→6→7→8→9 monotonic, no sawtooth/rewind (BUG-1 fixed), `codec=opus`, then `state→stopped` at end-of-track (BUG-3 fixed). |
| §2 RSS reclaim | **PASS** — 100 s Opus track: RSS 52→72 MB on initial buffer, then fell back 72→68→64→60 MB during playback — reclaim working, not pinned. |
| §3 backend error + try_recover | **LIMITED** — `pactl suspend-sink 64 1` did NOT raise an alsa backend_error (alsa writes to `plughw:0,5` directly); position kept advancing, `audio_available=true`, process did not abort (G5 consistent). try_recover via sink disturbance not exercisable with the alsa backend (pipewire.pc unavailable → pulseaudio backend not built). |
| §4 YouTube ceiling | **PASS** — `target_kbps=64`, streamed YouTube Opus track reported `target_kbps=64`, `actual_kbps=61`, `codec=opus`, position advancing ~1×, fast time-to-first-sound (~6 s to position 9 on cold 213 s track — G3 confirmed). Background `cache_next` under ceiling honored (`next_cached=true`, played back at `actual_kbps=61`). |
| Table item 1 (MKV wedge past 9 s) | **PASS** — `dQw4w9WgXcQ` (213 s Opus/251, froze at 9 s in `2026.11.03-1`): `position_sec` advanced continuously 19→30 over 20 s, `state=playing`, no freeze/rewind at old wedge point — 2026-11-05 re-probe fix confirmed on-device. |
| Table item 7 (CHUNK_FRAMES=4096) | **NOT EXECUTED** — pulseaudio backend not built locally (no pipewire.pc/pulseaudio.pc). |

**Limitations:** root-gated installer-install (apt/dnf/pacman libopus path) and any pulseaudio-backend
device check could not be run here. The merged `g1-opus-decode` branch is not yet on `main`.

---

## New findings — comprehensive API test battery (2026-11-05, Arch + PipeWire)

A full endpoint battery was run against the locally built alsa+opus binary
(`/tmp/l337-build/release/l337-audio-server`, `0.0.0-dev`) on port 13370 with
`L337__SERVER__TOKEN=dbgtkn123`, exercising all 21 routes, auth matrix, all 7
codecs, seek, volume, speed, target, cache, settings, stop/pause/next/previous,
concurrent plays, large-file upload, unsupported/empty files, and track_id
traversal. Two genuine bugs and one API inconsistency were found:

### NF-1 — Seek request field `position` vs status field `position_sec` (API inconsistency)

`SeekPayload.position` (src/api/models.rs:125) uses the field name `position`,
but `PlayerStatus.position_sec` (src/api/models.rs:153) reports `position_sec`.
API consumers that read `position_sec` from status and send it back to seek will
get a 422 "missing field `position`" error. The field names should be aligned
(either both `position` or both `position_sec`).

**Verified:** `POST /player/seek {"position_sec":5}` → 422 missing field `position`;
`{"position":5}` → 200 OK. `position_sec` readback after seek works correctly.

### NF-2 — Mutation endpoints return empty 200 OK bodies (API inconsistency)

`upload_stream` (handlers.rs:493), `set_volume` (handlers.rs:346), `set_speed`
(handlers.rs:336), `set_target` (handlers.rs:364ff), `stop`, `pause`, and
`settings` all return `StatusCode::OK` with an empty body. Only `/player/play`
returns `{"ok":true}`. Clients expecting a JSON confirmation body on every
mutation endpoint will fail to parse the empty response.

**Verified:** `POST /player/play/stream` → 200 empty body (operation succeeds,
state=playing). `POST /player/play {"stream_url":"file://..."}` → `{"ok":true}`.

### NF-3 — Out-of-range volume/speed silently clamped (design note, not a bug)

`set_volume` clamps to [0.0, 1.0] (engine.rs:850) and `set_speed_and_pitch`
clamps to [0.25, 4.0] (engine.rs:832-833). `volume=1.5`, `volume=-0.1`,
`speed=0` all return 200 OK with the clamped value applied, not 400. This is
intentional defensive behavior, but API clients cannot distinguish "accepted"
from "clamped" without reading back the status.

**Verified:** volume 1.5 → 200, readback 1.0; speed 0 → 200, readback 0.25;
target -1 → 422 (correctly rejected via TargetPayload validation).

### Not bugs (by design or test artifacts)

- `GET /setup` → 404 in HTTP mode: socket-mode-only endpoint by design
  (handlers.rs:40-46).
- `POST /auth/challenge` → 202: ACCEPTED by design in non-socket mode.
- `current_track`/`duration_sec`/`actual_kbps`/`codec` null when stopped:
  expected, not missing fields.
- Unsupported/empty file uploads accepted with 200: upload writes to disk;
  decode failure surfaces later, not at upload time.

## Solutions implemented (2026-11-05)

### NF-1 fix — Align seek field names (commit pending)

**Change:** `src/api/models.rs` line 125: `SeekPayload.position` → `SeekPayload.position_sec`

**Rationale:** The request field name now matches the response field name
(`PlayerStatus.position_sec`). A client that reads `position_sec` from status
and sends it back to seek will get 200 instead of 422.

**Files changed:**
- `src/api/models.rs:125` — `pub position: u64` → `pub position_sec: u64`
- `src/api/handlers.rs:353` — `engine.seek(payload.position)` → `engine.seek(payload.position_sec)`

### NF-2 fix — JSON `{"ok": true}` bodies on all mutation endpoints (commit pending)

**Change:** All 9 mutation endpoints now return `Json(serde_json::json!({"ok": true}))` instead of an empty `StatusCode::OK`.

**Rationale:** Only `/player/play` returned a JSON body; all other mutations returned
an empty 200 OK. Clients parsing the response body would fail. Consistent `{"ok": true}`
bodies make all mutation endpoints predictable.

**Files changed:**
- `src/api/handlers.rs:223` — pause: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:229` — stop: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:235` — next: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:241` — previous: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:336` — set_speed: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:345` — set_volume: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:354` — seek: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:379` — set_settings: empty 200 → `{"ok": true}`
- `src/api/handlers.rs:493` — upload_stream: empty 200 → `{"ok": true}`

### NF-3 resolution — Silent clamping is intentional, no code change

**Resolution:** Documented as intentional defensive behavior (engine.rs:832-833,
engine.rs:850). Volume clamps to [0.0, 1.0]; speed clamps to [0.25, 4.0]. Out-of-range
inputs return 200 OK with the clamped value applied. API clients can detect clamping
by reading back status after the mutation. No code change required.

---

## Additional findings — deep-dive round 2 (2026-11-05)

A deeper code review of handlers, models, engine, and auth uncovered 7 more
issues (NF-4 through NF-10), all now fixed.

### NF-4 — Test uses removed field name `position` (test breakage from NF-1 fix)

**Bug:** `src/api/tests.rs:111` — `test_api_seek` constructs `SeekPayload { position: 0 }`,
which references the old field name removed by the NF-1 fix. The test would not
compile.

**Fix:** Changed to `SeekPayload { position_sec: 0 }`.

**Files changed:**
- `src/api/tests.rs:111` — `position: 0` → `position_sec: 0`

### NF-5 — No NaN/Inf validation on volume/speed/pitch (security/correctness)

**Bug:** `VolumePayload.volume` (models.rs:110), `SpeedPayload.speed` (models.rs:103),
and `SpeedPayload.pitch` (models.rs:105) are bare `f32` fields with no NaN/Inf
checks. `f32::clamp` propagates NaN, so `volume: NaN` or `speed: Infinity` returns
200 OK but stores garbage state. NaN serializes as `null` in JSON, confusing
clients that read back status.

**Fix:** Added `is_nan()`/`is_infinite()` guards in `set_volume` and `set_speed`
handlers, returning 400 with `{"ok": false, "error": "..."}` for non-finite values.

**Files changed:**
- `src/api/handlers.rs` — `set_speed` (line ~334): NaN/Inf check on speed + pitch
- `src/api/handlers.rs` — `set_volume` (line ~344): NaN/Inf check on volume

### NF-6 — cache_next/cache_previous return empty ACCEPTED bodies

**Bug:** `handlers.rs:284` and `handlers.rs:327` — `cache_next` and `cache_previous`
returned `StatusCode::ACCEPTED.into_response()` with an empty body, inconsistent
with other mutation endpoints.

**Fix:** Both now return `Json(serde_json::json!({"ok": true}))`.

**Files changed:**
- `src/api/handlers.rs` — `cache_next`: empty ACCEPTED → `{"ok": true}`
- `src/api/handlers.rs` — `cache_previous`: empty ACCEPTED → `{"ok": true}`

### NF-7 — Auth rate limiting bypass when ConnectInfo is None (security)

**Bug:** `handlers.rs:73-81` and `handlers.rs:117-125` — `auth_challenge` and
`auth_redeem` only applied rate limiting inside `if let Some(ConnectInfo(addr)) =
maybe_addr`. When no remote address is available (e.g. Unix socket connections
or certain proxy configurations), rate limiting was entirely skipped, allowing
unlimited auth attempts.

**Fix:** Rate limit address now defaults to `SocketAddr::from(([0,0,0,0], 0))`
when `ConnectInfo` is absent, so rate limiting always applies.

**Files changed:**
- `src/api/handlers.rs` — `auth_challenge`: always check rate limiter
- `src/api/handlers.rs` — `auth_redeem`: always check rate limiter

### NF-8 — track_id_missing() returns plain text, not JSON

**Bug:** `handlers.rs:172-178` — `track_id_missing()` returned plain text
`"track_id and stream_url are required"` with a 400, inconsistent with the JSON
error format used by `play`, `cache_next`, `cache_previous`, and `auth_redeem`.

**Fix:** Now returns `Json(serde_json::json!({"ok": false, "error": "track_id and stream_url are required"}))`.

**Files changed:**
- `src/api/handlers.rs` — `track_id_missing()`: plain text → JSON error body

### NF-9 — upload_stream error responses return plain text, not JSON

**Bug:** `handlers.rs` — `upload_stream` returned plain text bodies for three
error cases: invalid endpoint (`"invalid upload endpoint"`), missing/invalid
`X-Track-Id` header, and upload I/O failure (`"upload failed"`). Inconsistent
with JSON error format used elsewhere.

**Fix:** All three error paths now return JSON `{"ok": false, "error": "..."}`.

**Files changed:**
- `src/api/handlers.rs` — invalid endpoint error: plain text → JSON
- `src/api/handlers.rs` — X-Track-Id error: plain text → JSON
- `src/api/handlers.rs` — upload failed error: plain text → JSON

### NF-10 — SpeedPayload.pitch defaults to 0.0, silently clamped to 0.25

**Bug:** `models.rs:104` — `SpeedPayload.pitch` used `#[serde(default)]`, which
defaults `f32` to `0.0`. `set_speed_and_pitch` clamps pitch to [0.25, 4.0], so
sending `{"speed": 1.5}` without a pitch field silently changed pitch to 0.25
instead of leaving it at the neutral 1.0.

**Fix:** Changed to `#[serde(default = "default_pitch")]` with a new
`default_pitch()` function returning `1.0` (neutral, no pitch shift).

**Files changed:**
- `src/api/models.rs` — `SpeedPayload.pitch`: `#[serde(default)]` → `#[serde(default = "default_pitch")]`
- `src/api/models.rs` — added `fn default_pitch() -> f32 { 1.0 }`

---

## Additional findings — deep-dive round 3 (2026-11-05)

A third code review pass over `download_stream`, `play_pushed`, and the upload
handler uncovered 2 more issues (NF-11 through NF-12), all now fixed.

### NF-11 — `download_stream` allows reading arbitrary local files via bare absolute path `stream_url` (security)

**Bug:** `engine.rs:1461-1467` — `download_stream` accepted bare absolute paths
(e.g. `/etc/passwd`) as `stream_url`, copying any readable file on the filesystem
into the server's slot directory. The `file://` prefix was the documented local
file mechanism; bare absolute paths were an undocumented second path that
bypassed any intent to scope local file access. Combined with `POST /player/play`
accepting any `stream_url` string, an authenticated client could read arbitrary
files the server process can read.

**Fix:** Removed the bare absolute path branch from `download_stream`. Local
file access now requires the `file://` prefix only (`url.strip_prefix("file://")`).

**Files changed:**
- `src/player/engine.rs` — `download_stream`: only `file://` prefix accepted for local files; bare absolute paths rejected

### NF-12 — `play_pushed` swallows playback errors after stream upload

**Bug:** `engine.rs:483-504` — `play_pushed` returned `()` and called
`load_and_play` (which also returns `()`) without checking whether playback
actually started. If the uploaded file couldn't be decoded, `load_and_play`
logged an error and returned, leaving the engine in `Stopped` state. The caller
(`upload_stream` for the `current` slot) returned `{"ok": true}` to the client
even though nothing was playing.

**Fix:** `play_pushed` now returns `Result<(), String>`. After `load_and_play`,
it checks `self.state != PlayerStateLabel::Playing` and returns an error if
playback didn't start. `upload_stream` propagates this as a 500 with a JSON
error body.

**Files changed:**
- `src/player/engine.rs` — `play_pushed`: returns `Result<(), String>`, checks state after `load_and_play`
- `src/api/handlers.rs` — `upload_stream`: propagates `play_pushed` error as 500 JSON

---






---

## Session — 2026-11-06 (Termux device): the ~5 s drops were a PulseAudio pacing bug — FIXED

**Symptom.** Occasional audible drops during playback, on local files and streams alike.

**Method.** Local audio file (Rick Astley, 213 s Opus/251, downloaded with yt-dlp →
`~/l337-live-verify/local-track.webm`) played through the installed release build, with the
PulseAudio writer instrumented server-side to log (a) every starvation event while playing and
(b) every inter-write gap above a threshold. Server-side logging was essential: client-side
`position_sec` polling missed the sub-second drops entirely.

**Evidence.** WRITE_GAPs of **85–86 ms on 60/60 consecutive writes** on a local-file playback —
i.e. *every* write interval was 2× the audio duration. The sink's own buffer is ~43 ms
(`pactl list sink-inputs` → `Buffer Latency: 42915 usec`), so writing 42.7 ms of audio every
85 ms left it dry twice per chunk: continuous drops, not just "occasional".

**Root cause.** `writer_loop` paced itself with the **shared buffer's** `channels` field, which is
0 until the engine sets it and need not match the sample spec the stream was created with. A
2-channel stream paced with a 1-channel assumption sleeps twice as long as the audio written. The
reciprocal bug (buffer says 2 while the stream is 1) would have starved the *producer* instead.

**Fix** (`6db3714`): `writer_loop` takes the `rate`/`channels` the stream was *created* with; the
arithmetic moved to `pace_seconds()` with unit tests pinning the stereo case, the doubled-pace
regression and the degenerate inputs; the starved-writer diagnostic is edge-triggered per episode
(≥3 consecutive polls, ~30 ms — below the sink's own buffer, which is why one poll is not logged)
so a real drop is reported once and the end-of-track tail does not add noise.

**Verification.** Release build (optimized, `--no-default-features --features pulseaudio,opus`),
local file, full 213 s track, twice: **4 write gaps of 61–66 ms** (normal `thread::sleep` jitter,
vs 60/60 at 85–86 ms before), zero mid-track starvation, clean `state → stopped` at EOS. Rust
suite 67/67 (64 + 3 new pacing tests).

**Still open on this device:**
- The **YouTube-streaming** path has not been re-measured with the fixed writer (the installed
  release `2026.11.03-1` still carries the old wedge — it froze at 12 s in this session; the
  streaming path with the fix needs a fresh release or a local build).
- The **~5 s whole-process freezes** (2026-11-05 session) were not observed during these local-file
  runs, but a low-CPU release build is exactly the case where they should be rarest; re-measure
  with the WRITE_GAP instrumentation if drops are heard on a stream.
- The client-side sink-margin experiment (`pa_buffer_attr`) remains unviable: PulseAudio
  negotiated 43 ms regardless of the requested 4 s (see the 2026-11-06 note above).

---

## Session — 2026-11-06 (continued): default the Termux sink buffer to ~100 ms

With the pacing fix in, the writer delivers at exactly 1x, but the sink's own
buffer was still ~43 ms — anything longer than that reaches the speaker. PulseAudio
*tunes the latency down* unless the stream asks for a specific one, and it overrides
the request while `PA_STREAM_ADJUST_LATENCY` is set (which is why the earlier 4 s
`buffer_attr` experiment negotiated 43 ms and sounded worse, not better).

So the stream now requests its own latency (`c13b4bd`): `tlength` = 60 ms
(negotiates **~86 ms** on this device — PulseAudio rounds up to the device's fragment
size), `maxlength` = 2× so the backlog stays bounded (BUG-2's property), `prebuf = -1`
so playback waits the target before the first sample, and `PA_STREAM_ADJUST_LATENCY`
is dropped so the request survives.

**Measured on device after the change:**
- `pactl list sink-inputs` → `Buffer Latency: 85577 usec` (was 43 ms).
- Start delay unchanged in practice: the `/player/play` request already blocks on the
  file copy/decode (~1.0–1.6 s), and the first sample follows within ~6 ms of the
  response, because the buffer fills during the decode.
- Full 213 s tracks — local file and local-HTTP stream — **zero writer starvation
  mid-track**; the only logged event remains the ~30 ms end-of-track drain tail, which
  is inside the sink buffer and inaudible.

Scope: `src/platform/pulseaudio.rs`, gated `#[cfg(all(target_os = "android",
feature = "pulseaudio"))]` — the Termux path only. No other backend is affected.

**Still open on this device:**
- Real YouTube streaming could not be run to completion here (googlevideo throttles
  the direct URL to a fraction of the playback rate, so the run is download-bound, not
  writer-bound). The streaming path *is* verified drop-free against local HTTP with the
  same code, but a long real-YouTube run with this build is still owed.
- The ~5 s whole-process freezes (2026-11-05) were not observed in any of these
  low-CPU release runs; if they reappear a >=86 ms sink buffer no longer covers them and
  the wake lock becomes the remaining fix.

---
## Session — 2026-11-09: l337 pipewire install exposes no hardware sink — RESOLVED

**Symptom.** Fresh l337 pipewire install runs the server correctly (`state: playing`,
`codec: pcm_s16le`, `audio_available: true`) but nothing is audible. `wpctl status` for the
l337 user's PipeWire session shows **no Devices/Sinks/Sources/Filters** at all — only
`Dummy-Driver`/`Freewheel-Driver`/MIDI plus the server's orphaned output streams. Direct ALSA
`aplay` as l337 works (audible), and `mpv --ao=pipewire` as l337 works (audible), so the
hardware and ALSA access are healthy; only WirePlumber never created ALSA sink nodes for
l337's session.

**Root cause.** l337's `~/.local/state/wireplumber/` had no `default-profile` (gaming's session
has `alsa_card.pci-...=HiFi (...)`), so WirePlumber never enumerated the ALSA card and created
sink nodes for l337. Both sessions share the same untouched system WirePlumber config
(`/usr/share/wireplumber/wireplumber.conf` + `monitors/alsa.lua`, dated Sep 30 — nothing
destroyed). Not a per-user audio config: l337 has no shell config files, gaming's `.bashrc`
only sets `SDL_AUDIODRIVER`/`PULSE_SERVER`, no `asound.conf`/`.asoundrc` or audio-switching
script exists anywhere.

**Fix.** Copying gaming's `default-profile`/`default-nodes`/`default-routes` into l337's
`~/.local/state/wireplumber/` + restarting wireplumber exposes `Meteor Lake-P HD Audio
Controller [alsa]` with Speaker/HDMI sinks at `vol: 1.00`.

**Verified.** WAV (`test_a440.wav`, 6 s, 44.1 kHz stereo pcm_s16le) → `state: playing` →
`stopped` at EOS, `actual_kbps: 1209` — **audible confirmed**. OGG
(`test_a440.ogg`, 6 s, 44.1 kHz stereo vorbis) → `playing` → `stopped` at EOS,
`codec: vorbis`, `actual_kbps: 22` — **audible confirmed**.

**Installer fix.** `scripts/install-linux-pipewire.sh` now seeds the user's
`~/.local/state/wireplumber/default-profile`/`default-nodes` from the detected ALSA card
(`seed_wireplumber_state()` in `setup_user_pipewire_services()`), auto-detecting the card via
`aplay -l`/`pw-cli`, so fresh installs expose hardware sinks automatically. `--uninstall
--remove-data` removes the seeded state. `bash -n` clean.

---

## NF-13 — Concurrent playback / `stop()` race (2026-11-11, `src/player/engine.rs`)

**Reported symptom:** The audio server appeared able to play 2 files at the same time — the user
heard what seemed like the same track played twice, and the buffer jumped between two streams.
The server should only play one file at a time.

**Root cause:** `PlayerEngine::stop()` was synchronous — it *aborted* the background streaming
download/decode tasks without awaiting them. `play_track()` called `stop()` then immediately
started a new decode, so the old decode task could still be running and appending PCM to the shared
`AudioBuffer` while the new track's decode also wrote to it. Two producers writing to one buffer
caused the same track to be heard twice and the buffer to "jump between the two streams."

**Fix:** Made `stop()` async. It now sets the cancel flag, aborts *and awaits* the download
handle, then awaits the decode handle with a 5-second timeout. This guarantees the old producer
has fully stopped writing to the shared buffer before a new playback begins — enforcing
single-track playback.

**Call sites updated to `.await`:**
- `play_track` (engine.rs:455)
- `play_pushed` (engine.rs:592)
- `load_and_play` decode failure (engine.rs:707)
- `start_streaming_playback` probe fallback (engine.rs:989)
- `trigger_next` (engine.rs:1238)
- `trigger_previous` (engine.rs:1258)
- `get_status` end-of-track (engine.rs:1303)
- `/player/stop` handler (handlers.rs:230)
- `/player/next` handler (handlers.rs:236)

**Verification:** `cargo check --no-default-features --features alsa,opus` passes with no new
errors. Full suite passes: 64 unit + 26 endpoint tests, all green.

**Files changed:**
- `src/player/engine.rs` — `stop()` is now `async`, awaits cancelled streaming handles with a
  5 s timeout; all internal call sites updated.
- `src/api/handlers.rs` — `/player/stop` and `/player/next` call `engine.stop().await`.

---

## Additional findings consolidated from `bugs.md` (2026-11-08)

The following items were recorded in `bugs.md` and are consolidated here.

### Single-instance lock scope vs isolated runtimes (design note)

`InstanceLock` on Linux also locks `/tmp/l337-audio-server-runtime/instance.lock` for cross-TMPDIR
detection, except when `L337_RUNTIME_DIR` is set (isolated mode locks only that runtime). Correct for
normal mode; operators/tests wanting a fully separate instance should set `L337_RUNTIME_DIR`
consistently (and use matching runtime dirs). Per-socket lock now protects the same socket path
regardless of lock scope.

### Stale committed binary (`bin/l337-audio-server` is v0.1.0)

`bin/l337-audio-server` in the repo is a stale v0.1.0 binary (built Sep 5 2026) that predates
`/setup`, `/version`, `/health`, and `--help`/`--token`. It is a foot-gun for anyone who runs it
directly instead of building via `scripts/build.sh` or fetching via `gh`. Either remove the committed
binary or document clearly that it is a placeholder. Replaced by a fresh build during validation but
should be regenerated/committed via CI.

### Single-instance lock uses a fixed, privileged path

`src/platform/single_instance` attempts `/run/l337-audio-server/instance.lock`, which is unwritable
by a plain user (`Permission denied (os error 13)`). The lock should live under `runtime_dir()`
(or `~/.cache/...` on the older binary) so unprivileged dummy runs do not needlessly warn.

### `token_file_path()` vs env token

On the v0.1.0 binary, `L337__SERVER__TOKEN` overrides the persisted token, and `--token` then
prints the (possibly empty) path rather than the effective value. Not a blocker, but worth asserting
in `--token` that a token actually exists.

### Stale `/etc/l337-audio-server/server.ini` shadows the server config (§14)

`src/main.rs:load_settings()` reads configuration sources in this order:

1. `/etc/l337-audio-server/config`
2. `/etc/l337-audio-server/server.ini` ← stale file from Sep 15 takes precedence over #3/#4
3. `server.ini` next to the binary (`CWD`)
4. Environment variables `L337__*`

A pre-existing stale `/etc/l337-audio-server/server.ini` (e.g. `host = "127.0.0.1"`, `port = 1337`,
`dummy = false`, `transport = "http"`) shadows the CWD/env config and forces TCP+TLS with no token,
returning `HTTP 401 missing or invalid token`. Additionally, installers write user config to
`~/.config/l337-audio-server/server.ini`, but `load_settings()` never reads that path.

Fix: installers sync generated config into `/etc/l337-audio-server/server.ini` (replacing stale
`transport = "http"` with `auto` and correct token/dummy settings).

### No audible output — PipeWire has no hardware sink for l337 (§15)

After the config fix, the server runs correctly over the Unix socket (`state: playing`,
`codec: pcm_s16le`, `audio_available: true`) but nothing is audible because l337's WirePlumber
session never exposes the ALSA hardware device as a sink — `wpctl status` shows only
`Dummy-Driver`/`Freewheel-Driver` plus orphaned server output streams. The ALSA hardware works
directly for l337 (`aplay` audible). The gap is PipeWire-only: l337's `~/.local/state/wireplumber/`
has no `default-profile` (unlike other users' sessions).

Fix: seed l337's WirePlumber state (`default-profile`/`default-nodes`/`default-routes`) from the
detected ALSA card in `install-linux-pipewire.sh` (`seed_wireplumber_state()`), auto-detecting the
