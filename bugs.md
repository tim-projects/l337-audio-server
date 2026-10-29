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

## Left to verify (needs a real backend / network)
- `position_sec` advancing ~1× and RSS falling back after playback (no writer in
  dummy mode).
- YouTube playback against a reachable YouTube (this sandbox gets
  "This video is unavailable"), so `actual_kbps` clamping and background-cache
  sizing under a real ceiling are unverified.
- A stock host *without* libopus reaching the new installer fail-gate (the gate
  itself is verified, only the package-manager install path is not).
