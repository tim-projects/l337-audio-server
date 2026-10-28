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
- [ ] `install-systemd.sh` / `install-linux-alsa.sh` / `install-linux-pipewire.sh`: detect the binary's `DT_NEEDED libopus` and install the runtime package (`libopus0` / `opus` / `opus`) per PM, with a clear warning if unavailable.
- [ ] Add a runtime dependency check (e.g. `readelf -d` or `ldd`) in the Linux installers so a missing lib is surfaced at install time, not first boot.
- [ ] `build.yml`: add a post-build verify (both Linux jobs) that the binary's `NEEDED` set includes `libopus.so`/`libopus.so.0`.
- [ ] `release-info.json` generator / README: record `libopus` as a required shared library for the Linux assets.
- [ ] Verified: a stock Linux host can install + start the release server (dummy or real).

---

## 2. WARNING — G4 pause/reclaim + window cap migrated to PulseAudio only; other 5 backends still trip on a long track

- **Where:** `src/platform/linux.rs:98`, `src/platform/alsa.rs:189`, `src/platform/android.rs:167`, `src/platform/macos.rs:78`, `src/platform/windows.rs:125` (all still `buf.pcm.len() > buf.max_bytes / 4`, no reclaim) vs `src/platform/pulseaudio.rs:226/270` (uses `exceeds_window()`/`reclaim_consumed()`).
- **Problem:** The branch redefined `AudioBuffer::max_bytes` as the *unconsumed* window (24 MiB) and added `exceeds_window()`/`reclaim_consumed()`, but only PulseAudio uses them. On the default desktop PipeWire backend (and ALSA/AAudio/macOS/Windows), `pcm.len()` keeps growing with track length and nothing reclaims, so a track longer than ~6.3 M samples (~65 s stereo @48 kHz, or the whole file path) clears the buffer + raises `backend_error` → `try_recover` loop. "Memory independent of track length" holds only on PulseAudio.
- **Suggestion:** replace the raw checks in all five writers with `buf.exceeds_window()` and call `buf.reclaim_consumed()` at each writer's checkpoint, exactly as pulseaudio.rs.

**Checklist**
- [ ] `linux.rs`: cap → `buf.exceeds_window()`; call `buf.reclaim_consumed()` after `read_pos` advances (checkpoint).
- [ ] `alsa.rs`: same (note ordering — reclaim after the lock-taken copy, no double-lock).
- [ ] `android.rs` (AAudio): same.
- [ ] `macos.rs` + `windows.rs`: migrate if the check is on a live path; if placeholder/Noop, confirm the check is unreachable and skip with a note.
- [ ] Confirm no backend drains/i-set state under a lock held across reclaim (verify lock scope in each).
- [ ] Verify: `grep src/platform -e 'pcm.len() > buf.max_bytes'` returns nothing.

---

## 3. WARNING — File/fMP4/cached path keeps the whole decoded track resident

- **Where:** `src/player/engine.rs:522` (`load_and_play`: `buf.pcm = playback_pcm`), also `engine.rs:283` (seek_and_resume), `engine.rs:810` (seek); `decode_to_pcm` growing-Vec `extend_from_slice`.
- **Problem:** The entire decoded + resampled track is assigned in one shot, so a long track pins its full PCM (≈1.1 GiB for 60 min stereo @48 kHz, ×2–2.5 with the whole-file `bytes` + geometric realloc of the decode Vec). This defeats the bounded-window goal on every backend and risks OOM/lowmemorykiller on device for hour-long tracks.
- **Suggestion:** ideally decode the local file incrementally into the throttled `audio_buffer` producer (a "disk file" `MediaSourceStream` + the streaming packet loop already exist); at minimum, pre-reserve/shrink so capacity tracks the *unconsumed* window rather than track length.
  - **Scope note (shortcut):** full incremental file decode is a larger, harder-to-verify-on-device change; we adopt the "at minimum" path now and document the residual peak.

**Checklist**
- [ ] `common.rs::reclaim_consumed`: after `drain`, keep capacity bounded (`shrink_to_fit` gated to avoid per-chunk thrash, e.g. only when dropped bytes dominate).
- [ ] `decode_to_pcm`: pre-reserve the output Vec (`Vec::with_capacity`) so it doesn't realloc geometrically.
- [ ] `load_and_play`: (`bytes` whole-file copy) — note as residual; add a `shortcut:` comment: whole-track decode peak remains, upgrade path = incremental file decode via the streaming producer.
- [ ] Verify: on device, a long local/fMP4 track plays to completion and RSS falls back as it plays (not pinned).

---

## 4. WARNING — Background cache ignores the runtime ceiling from `POST /player/target`

- **Where:** `src/api/handlers.rs:254`, `:296` (`engine.default_target_kbps()` = config) vs `src/player/engine.rs:346` (`play_track` uses `self.runtime_target_kbps`).
- **Problem:** After `POST /player/target {"target_kbps":64}`, live playback uses 64 but `cache_next`/`cache_previous` resolve with the config default (0 → `bestaudio`), so `next`/`prev` are downloaded above the active ceiling and `PlayerStatus` reports `target_kbps=64` alongside a higher `actual_kbps`.
- **Suggestion:** use `engine.current_default_target_kbps()` in both handlers — better, one `engine.resolve_track_ceiling(&Track)` so the precedence rule exists once.

**Checklist**
- [ ] `handlers.rs::cache_next` + `cache_previous`: resolve via `engine.current_default_target_kbps()`.
- [ ] (Optional, preferred) add `PlayerEngine::resolve_track_ceiling(&Track)` and route `play_track`, both cache handlers, and `set_target` through it so the precedence lives in one place.
- [ ] Test: set runtime `target_kbps=64`, then cache next/prev, then assert the persisted/next slice uses a ≤64 selector.
- [ ] Verify: no remaining `resolve_ceiling(...default_target_kbps())` in `src/api/handlers.rs`.

---

## 5. WARNING — Unknown `quality` string is rejected (contradicts the "never errors" contract)

- **Where:** `src/api/models.rs:9-10` (`Quality` enum, `rename_all="lowercase"`, no unknown fallback) vs `resolve_ceiling` doc at `models.rs:47`.
- **Problem:** Any unrecognized string (`"ultra"`, `"veryHigh"`) fails serde deserialization at the axum `Json` extractor → 400/422 before `resolve_ceiling` runs. Only *absent* (`#[serde(default)]` + `Option<..>`) is non-erroring. The documented contract ("unknown/absent quality never errors — it is skipped") only holds when the key is omitted.
- **Suggestion:** add a tolerant deserializer for the `quality` fields (unknown → `None`), or deserialize into `String`/`Option<String>` and match; add a test for unknown → server default.

**Checklist**
- [ ] `models.rs`: add a `deserialize_with` helper for `quality` (Track + TargetPayload) that maps unknown/malformed strings to `None`.
- [ ] Unit test: `quality: "ultra"` (and casing variants) deserializes to `None` → ceiling = server default.
- [ ] Integration check: `POST /player/play {"quality":"ultra"}` returns 200 and plays at `default_target_kbps`/best.
- [ ] Keep the `resolve_ceiling` doc accurate.

---

## 6. WARNING — `lossless` not enforced; `needs_bitexact()` dead outside tests

- **Where:** `src/api/models.rs:39` (`needs_bitexact`), `src/player/engine.rs::yt_dlp_format_selector` (Lossless → 0 → `bestaudio`, often Opus/lossy), `PlayerStatus.codec`.
- **Problem:** `Quality::Lossless.ceiling_kbps() == 0` → `bestaudio` → yt-dlp routinely resolves to Opus (lossy); nothing calls `needs_bitexact()`, so bit-exactness is never enforced/degraded/flagged. The documented promise ("bit-exact pass-through; for lossy-only sources degrades to best and says so") is unimplemented; a lossless request silently returns lossy audio.
- **Suggestion:** either wire `needs_bitexact()` into selection (prefer a lossless format; return an explicit degradation flag if none) **or** remove it and make the docs honest — server pass-throughs (never re-encodes), and `PlayerStatus.codec` reports whether delivery is actually lossless.
  - **Scope note:** the server has no transcode-down path, so "never a lossy re-encode" is already satisfied; the real gap is the overclaim, so we remove the dead getter + align docs (the honest, minimal fix).

**Checklist**
- [ ] Remove `Quality::needs_bitexact` and its unit test (dead in product).
- [ ] Update the `Quality`/`Lossless` doc to state: `lossless` maps to ceiling 0 (no ceiling) like `best`; the server pass-throughs and never re-encodes; `PlayerStatus.codec` reports the actual delivered encoding.
- [ ] (Optional, future) real bit-exact enforcement: prefer a lossless selector + degrade-with-flag; tracked separately.
- [ ] `cargo build` no longer warns on `needs_bitexact`; no reference remains in `src/`.

---

## 7. SUGGESTION — `CHUNK_FRAMES = 256` forces ~190 PulseAudio writes/sec

- **Where:** `src/platform/pulseaudio.rs:206`.
- **Problem:** With the new pacing (`sleep n/(channels*rate)`), a 256-sample chunk = ~5.3 ms at 48 kHz → ~188 `pa_stream_write`/sec (~375 @96 kHz), each a 1 KiB pool buffer + locks + IPC/wakeup. `pa_stream_write` is queued/async, so a bigger chunk is correctness-neutral — only the write cadence changes.
- **Suggestion:** raise `CHUNK_FRAMES` to 2048–4096 (≈40–80 ms), keeping the cap guard and pacing math unchanged.

**Checklist**
- [ ] `pulseaudio.rs`: `CHUNK_FRAMES = 4096`.
- [ ] Verify on device: audio still plays 1×, no underrun/overrun regression, lower CPU/wakeups (rough check via `top`/battery).

---

## 8. SUGGESTION — Termux libopus install/diagnostics + termux linkage verify + Linux matrix robustness

- **Where:** `scripts/install-android-termux.sh:363` (libopus check), `build.yml:337` (verify step), `build.yml:52` (concurrency/fail-fast + publish `continue-on-error`), `build.yml` alsa `pkg_deps` (unnecessary `libclang-dev clang`).
- **Problem:** (a) the libopus check hard-fails the install by default and, under `--no-install-deps`, only warns → then the binary can't load (`CANNOT LINK EXECUTABLE … libopus.so`), runit crash-loops it, and the final hints blame "an audio-backend failure" and suggest `dummy=true` (which can't help — `main()` never runs). (b) The termux verify step only greps `libpulse.so`, never the new `libopus.so`. (c) Linux matrices keep `fail-fast: true` and `publish-release` carries `continue-on-error`, so one variant failure cancels its sibling and skips the release while the run may still report green; the `libclang-dev clang` on alsa is unneeded.
- **Suggestion:** under `--no-install-deps`, route missing libopus into `NEEDS_USER`; distinguish a loader failure from backend failure in the service diagnostics; assert both `libpulse.so` and `libopus.so` in the termux verify step; `fail-fast: false` on the Linux matrices; drop `libclang-dev clang` from alsa `pkg_deps`.

**Checklist**
- [ ] `install-android-termux.sh`: missing libopus under `--no-install-deps` → `NEEDS_USER` (not just warn).
- [ ] `install-android-termux.sh`: after `sv up`, if the service log shows `CANNOT LINK EXECUTABLE` / `library "libopus.so" not found`, report a loader/dependency failure instead of the audio-backend `dummy=true` hint.
- [ ] `build.yml` termux verify step: require `libopus.so` as well as `libpulse.so`.
- [ ] `build.yml`: set `fail-fast: false` on the Linux `build-linux` + `build-linux-arm64` matrices (and consider failing publish-report when publish is skipped).
- [ ] `build.yml`: remove `libclang-dev clang` from the alsa `pkg_deps` (bindgen there comes from C-FFI crates that `--no-default-features` disables).
- [ ] Verify: trigger `target=all`; every target green with the opus-enabled Linux/Android assets.

---

## 9. SUGGESTION — Committed `libopus.so` stub blob can drift; armv7/riscv64 Opus capability unrecorded

- **Where:** `scripts/opus-stubs/libopus.so` (committed binary), `scripts/opus-stubs/opus-stub.c`, `.gitignore` negation, `build.yml:284` (armv7/riscv64 alsa jobs).
- **Problem:** An aarch64 ELF blob is committed with no CI regeneration/validation, so it can drift from `opus-stub.c` (only the linker would catch it; unauditable by diff). Separately, armv7/riscv64 release binaries build without Opus and nothing in `release-info.json`/README records which assets carry it.
- **Suggestion:** add a CI step that rebuilds the stub from `opus-stub.c` or asserts `Machine=AArch64`, `SONAME=libopus.so`, and the 4 symbols; record per-asset Opus capability in release metadata/README.

**Checklist**
- [ ] `build.yml`: a step for the termux job that asserts the stub `readelf -h/-d` (AArch64, SONAME `libopus.so`) and the 4 exported symbols.
- [ ] `build.yml` / README: comment + record that armv7/riscv64 assets have no Opus (source-agnostic ceiling still degrades + reports `actual_kbps`).
- [ ] (Optional) regenerate the stub in CI and fail on diff.

---

## 10. SUGGESTION — Duplicated stream stall/retry policy between the Opus and Symphonia streaming paths

- **Where:** `src/player/engine.rs:1699-1711` (Opus streaming `IoError` block) vs `src/player/engine.rs:1889-1901` (existing Symphonia block).
- **Problem:** The two copies share one "how long to tolerate a stalled/partial stream" policy (file-growth check, `stall_cycles > 20`, 200 ms) but live in separate branches; tuning one leaves Opus streams aborting after ~4 s of no growth under identical network conditions.
- **Suggestion:** factor into `fn wait_for_progress(path, &mut last_file_size, &mut stall_cycles) -> bool` and call it from both match arms.

**Checklist**
- [ ] Add `wait_for_progress` helper; call it in both `Err(Error::IoError(_))` arms.
- [ ] Behavior preserved for the Symphonia path; Opus path now shares the tolerance.
- [ ] Tests still pass; no shared-mutex lock held across the helper's sleep.

---

**Progress:** 0 / 10 fixed.
