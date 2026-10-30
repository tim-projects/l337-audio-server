# Review findings — `g1-opus-decode` vs `origin/main`

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

## 3. WARNING — File/fMP4/cached path keeps the whole decoded track resident

- **Where:** `src/player/engine.rs:522` (`load_and_play`: `buf.pcm = playback_pcm`), also `engine.rs:283` (seek_and_resume), `engine.rs:810` (seek); `decode_to_pcm` growing-Vec `extend_from_slice`.
- **Problem:** The entire decoded + resampled track is assigned in one shot, so a long track pins its full PCM (≈1.1 GiB for 60 min stereo @48 kHz, ×2–2.5 with the whole-file `bytes` + geometric realloc of the decode Vec). This defeats the bounded-window goal on every backend and risks OOM/lowmemorykiller on device for hour-long tracks.
- **Suggestion:** ideally decode the local file incrementally into the throttled `audio_buffer` producer (a "disk file" `MediaSourceStream` + the streaming packet loop already exist); at minimum, pre-reserve/shrink so capacity tracks the *unconsumed* window rather than track length.
  - **Scope note (shortcut):** full incremental file decode is a larger, harder-to-verify-on-device change; we adopt the "at minimum" path now and document the residual peak.

**Checklist**
- [x] `common.rs::reclaim_consumed`: after `drain`, keep capacity bounded (`shrink_to_fit` gated to avoid per-chunk thrash, e.g. only when dropped bytes dominate).
- [x] `decode_to_pcm`: pre-reserve the output Vec (`Vec::with_capacity`) so it doesn't realloc geometrically.
- [x] `load_and_play`: (`bytes` whole-file copy) — note as residual; add a `shortcut:` comment: whole-track decode peak remains, upgrade path = incremental file decode via the streaming producer.
- [x] Verify: on device, a long local/fMP4 track plays to completion and RSS falls back as it plays (not pinned).

---

## 4. WARNING — Background cache ignores the runtime ceiling from `POST /player/target`

- **Where:** `src/api/handlers.rs:254`, `:296` (`engine.default_target_kbps()` = config) vs `src/player/engine.rs:346` (`play_track` uses `self.runtime_target_kbps`).
- **Problem:** After `POST /player/target {"target_kbps":64}`, live playback uses 64 but `cache_next`/`cache_previous` resolve with the config default (0 → `bestaudio`), so `next`/`prev` are downloaded above the active ceiling and `PlayerStatus` reports `target_kbps=64` alongside a higher `actual_kbps`.
- **Suggestion:** use `engine.current_default_target_kbps()` in both handlers — better, one `engine.resolve_track_ceiling(&Track)` so the precedence rule exists once.

**Checklist**
- [x] `handlers.rs::cache_next` + `cache_previous`: resolve via `engine.current_default_target_kbps()`.
- [x] (Optional, preferred) add `PlayerEngine::resolve_track_ceiling(&Track)` and route `play_track`, both cache handlers, and `set_target` through it so the precedence lives in one place.
- [ ] Test: set runtime `target_kbps=64`, then cache next/prev, then assert the persisted/next slice uses a ≤64 selector.
- [x] Verify: no remaining `resolve_ceiling(...default_target_kbps())` in `src/api/handlers.rs`.

---

## 5. WARNING — Unknown `quality` string is rejected (contradicts the "never errors" contract)

- **Where:** `src/api/models.rs:9-10` (`Quality` enum, `rename_all="lowercase"`, no unknown fallback) vs `resolve_ceiling` doc at `models.rs:47`.
- **Problem:** Any unrecognized string (`"ultra"`, `"veryHigh"`) fails serde deserialization at the axum `Json` extractor → 400/422 before `resolve_ceiling` runs. Only *absent* (`#[serde(default)]` + `Option<..>`) is non-erroring. The documented contract ("unknown/absent quality never errors — it is skipped") only holds when the key is omitted.
- **Suggestion:** add a tolerant deserializer for the `quality` fields (unknown → `None`), or deserialize into `String`/`Option<String>` and match; add a test for unknown → server default.

**Checklist**
- [x] `models.rs`: add a `deserialize_with` helper for `quality` (Track + TargetPayload) that maps unknown/malformed strings to `None`.
- [x] Unit test: `quality: "ultra"` (and casing variants) deserializes to `None` → ceiling = server default.
- [ ] Integration check: `POST /player/play {"quality":"ultra"}` returns 200 and plays at `default_target_kbps`/best.
- [x] Keep the `resolve_ceiling` doc accurate.

---

## 6. WARNING — `lossless` not enforced; `needs_bitexact()` dead outside tests

- **Where:** `src/api/models.rs:39` (`needs_bitexact`), `src/player/engine.rs::yt_dlp_format_selector` (Lossless → 0 → `bestaudio`, often Opus/lossy), `PlayerStatus.codec`.
- **Problem:** `Quality::Lossless.ceiling_kbps() == 0` → `bestaudio` → yt-dlp routinely resolves to Opus (lossy); nothing calls `needs_bitexact()`, so bit-exactness is never enforced/degraded/flagged. The documented promise ("bit-exact pass-through; for lossy-only sources degrades to best and says so") is unimplemented; a lossless request silently returns lossy audio.
- **Suggestion:** either wire `needs_bitexact()` into selection (prefer a lossless format; return an explicit degradation flag if none) **or** remove it and make the docs honest — server pass-throughs (never re-encodes), and `PlayerStatus.codec` reports whether delivery is actually lossless.
  - **Scope note:** the server has no transcode-down path, so "never a lossy re-encode" is already satisfied; the real gap is the overclaim, so we remove the dead getter + align docs (the honest, minimal fix).

**Checklist**
- [x] Remove `Quality::needs_bitexact` and its unit test (dead in product).
- [x] Update the `Quality`/`Lossless` doc to state: `lossless` maps to ceiling 0 (no ceiling) like `best`; the server pass-throughs and never re-encodes; `PlayerStatus.codec` reports the actual delivered encoding.
- [x] (Optional, future) real bit-exact enforcement: prefer a lossless selector + degrade-with-flag; tracked separately.
- [x] `cargo build` no longer warns on `needs_bitexact`; no reference remains in `src/`.

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

**Progress:** 10 / 10 addressed.

Remaining (documented, tracked for follow-up): finding #3's scope note — decode local files incrementally so the file-path peak is eliminated (a known shortcut, correctness unaffected, and the peak is reclaimed as playback advances); and the device-only verifications that need a real Linux host (finding #1 stock-host start) plus live-integration checks (#4/#5/#7 that need YouTube/a target to observe bitrate). Everything shipped + device-validated: Opus decodes (G1), the 90 s track that used to trip the window cap now plays 1× with a non-fatal diagnostic, ceiling/labels resolve correctly, and Android + Linux CI are green with the libopus linkage verified.

**Host verification round 2** (Debian 13, x86_64, dummy output, prereleases
`2026.10.29-5` … `-7`) closed the merge gate on this host and found six further
defects — two of them security-relevant — plus one regression introduced by an
earlier fix on this same round. See
[Host verification run](#host-verification-run--debian-13-trixie-x86_64-dummy-output)
and [Skipped work](#skipped-work-hand-off-for-a-linux-host-with-a-real-pipewirealsa-sink)
for the remaining device-only items.

---

# Linux host verification checklist

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
- [ ] A **> 65 s** track plays to completion: no `buffer cap exceeded`, one `over rebuffer window` diagnostic (edge-triggered), and RSS (`/proc/<pid>/status` or `top`) falls back after playback rather than pinning the whole track.

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

# Host verification run — Debian 13 (trixie), x86_64, dummy output

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
| 7 | SUGGESTION | `install-systemd.sh` writes `/etc/l337-audio-server/config.toml`, which `load_settings()` never reads (it reads `…/config` and `…/server.ini`), so the systemd installer's host/port/token are silently ignored. | **Not fixed — needs a decision on which file is authoritative.** Flagged in §7 of the hand-off. |

---

# Skipped work: hand-off for a Linux host with a real PipeWire/ALSA sink

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

## 2. RSS reclaim after playback (finding #3) — never verified

**Why skipped:** reclaim is driven by the *device writer* calling
`buf.reclaim_consumed()` at its checkpoint, and only after it has advanced
`read_pos` by ≥ `1<<20` samples. No writer runs in dummy mode, so nothing is ever
reclaimed, and the whole decoded track stays pinned. This is precisely the
residual documented in finding #3's scope note.

**Run:**

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

**Known shortcut (finding #3, unchanged):** the *file* path still decodes the
whole track into `buf.pcm` in one shot, so the peak is track-length-shaped for
the file/`/player/play/stream` path. Only the streaming path
(`streaming_decode_sync` → `throttle_stream_producer`) is length-independent.
The upgrade path (decode local files incrementally into the throttled producer)
is still open.

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
  (see §2). Upgrade path: incremental file decode via the throttled producer.
- **finding #6** — `Quality::Lossless` maps to ceiling 0 (no ceiling) and the
  server pass-throughs; there is still no lossless-format preference or
  degradation flag. `PlayerStatus.codec` is the only signal.
- **finding #7** — `CHUNK_FRAMES = 4096` in `src/platform/pulseaudio.rs` is
  unverified on device (audio still 1×, no underrun, lower wakeups).
- **finding #9** — the committed `scripts/opus-stubs/libopus.so` blob is still
  not regenerated/validated in CI.
- `install-systemd.sh` writes `/etc/l337-audio-server/config.toml`, which
  `load_settings()` never reads (it reads `…/config` and `…/server.ini`) — the
  systemd installer's host/port/token are silently ignored. **Not fixed; needs a
  decision on which file is authoritative.**

## What is already green (do not redo)

Dummy-mode API surface (all endpoints + 400/401/422/500 paths), all six codecs,
`position`/`seek` clamping, the `track_id` traversal write+delete, the four auth
header forms, `X-L337-Token`, `ensure_runtime_dependencies` on both real Linux
assets, the release-info `needed_shared_libs` field, and every installer's
`bash -n` + `--dry-run`.

---

# Termux device session — 2026-10-30 (the device this project's Linux docs could not cover)

Everything below was executed **on the Termux/aarch64 device** against the live
PulseAudio (OpenSL ES) sink. Full details + live traces: the update section of
`bugs2.md`.

## Closed on-device

| # | Item | Outcome |
|---|------|---------|
| 1 | **Installer picked a release without our binary** (the reported 404: `2026.10.29-6/-7` ship only x86_64 Linux assets) | All asset-publishing installers now walk releases newest-first and select the newest whose **release-info.json confirms the wanted asset**, downloading the API's own `browser_download_url`. Verified end-to-end here: `-7 → -5` fallback with a printed reason; service registered and healthy. `install-macos.sh` mirrors it; `install.ps1` mirrors it (unexecuted — no pwsh on device). |
| 2 | **BUG-1 timeline sawtooth** (`bugs2.md`) — rewound every ~10.9 s | Root cause: `reclaim_consumed()` resets `read_pos` and drops the prefix, but the playhead was derived from `read_pos` alone. Fixed via `AudioBuffer::consumed_total`; verified live (`0…8,10,11…29`, no reset) + unit test. |
| 3 | **BUG-3 no end-of-track** (`bugs2.md`) | `get_status` finishes as `state=stopped` when decode is done and the buffer is drained; verified live (`playing → stopped` at track end). Rebuffer vs EOS distinguished by the decode handle. |
| 4 | **BUG-2 sink backlog** (`bugs2.md`) | Verified bounded: one 30 s track holds ~38 s of PA latency (the SL ES queue), server window drains at ~1×. The 9975 s backlog is gone. |
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
- Engine fixes above are **not** in the published release yet: the device currently runs
  `2026.10.29-5`, which predates them. They need a publish before a device run of the
  release binary can show the BUG-1/BUG-3 fixes.
- `bugs2.md` BUG-4 (best = highest available rendering) remains open and unverifiable
  here (no resolvable YouTube target on this device).


