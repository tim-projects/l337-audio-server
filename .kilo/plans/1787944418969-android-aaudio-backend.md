# Android AAudio Backend (Termux / future APK service)

## Context
The CI currently builds the Android binary with `cargo build ... --no-default-features`
(`build.yml:120`). With no `backend` feature, `PlayerEngine::new` selects
`NoopAudioBackend` (`src/player/engine.rs:59-62`), so the binary compiles and runs but
**produces no audio** — that is why "it doesn't work".

Goal: add a real `android` platform module that uses the NDK **AAudio** API via raw
`extern "C"` FFI (no new Rust crate). AAudio is the modern, low-latency, callback-based
NDK API and its `dataCallback` maps 1:1 onto the existing contract: the backend fills the
device buffer from `Arc<Mutex<AudioBuffer>>.pcm[read_pos..]` and advances `read_pos`.

Compatibility note (user question): a `aarch64-linux-android` binary using native AAudio is
fully compatible with later being bundled into an APK and launched from an Android `Service`
(Termux-style native process). No architectural change needed now for that future wrapping.

## Decisions
- **API**: raw AAudio NDK FFI, `#[link(name = "aaudio")]`, declare functions ourselves. No `oboe`/`cpal`/AAudio crate.
- **Min API**: bump `21` -> `26` (AAudio requires API 26; NDK r28b already ships `libaaudio`).
- **Feature wiring**: reuse existing `backend` feature. On Android, `backend` is enabled and
  `src/platform/android.rs` compiles (`cfg(all(target_os = "android", feature = "backend"))`).
  The `pipewire`/`coreaudio-rs`/`wasapi` deps are target-gated, so they are no-ops on Android.
- **Sample format / rate**: request `AAUDIO_FORMAT_FLOAT`, 48000 Hz, 2 channels — exactly what
  `PlayerEngine::new` already produces (`engine.rs:76-82`). Apply `*volume` software gain in callback
  (matches linux.rs; no `pactl`/hardware-volume step on Android).

## Files to change

### 1. `src/platform/android.rs` (NEW)
Implement `AudioBackend` + `AudioOutputStream` over AAudio:
- `extern "C"` declarations for: `AAudio_createStreamBuilder`, `AAudioStreamBuilder_setDirection`,
  `..._setFormat`, `..._setSampleRate`, `..._setChannelCount`, `..._setSharingMode`,
  `..._setPerformanceMode`, `..._setDataCallback`, `..._setBufferCapacityInFrames`,
  `AAudioStreamBuilder_openStream`, `AAudioStream_requestStart`, `AAudioStream_requestPause`,
  `AAudioStream_requestStop`, `AAudioStream_close`, `AAudioStreamBuilder_delete`,
  `AAudioStream_getSampleRate`, plus enums/constants (`AAUDIO_DIRECTION_OUTPUT`,
  `AAUDIO_FORMAT_FLOAT`, `AAUDIO_SHARING_MODE_SHARED`, `AAUDIO_PERFORMANCE_MODE_LOW_LATENCY`,
  `AAUDIO_CALLBACK_RESULT_CONTINUE`).
- `#[link(name = "aaudio")]`.
- Shared state struct `AndroidState { buffer: Arc<Mutex<AudioBuffer>>, volume: Arc<Mutex<f32>>, playing: Arc<AtomicBool> }`.
- `start_stream`: build stream (DIRECTION_OUTPUT, FLOAT, 48000, 2ch), set data callback with
  `userdata = Arc::into_raw(state.clone())`, `requestStart`. Return `AndroidAudioOutputStream`
  holding the `*mut AAudioStream` and the raw userdata ptr. Log `Starting AAudio stream ...`.
- `data_callback(stream, userdata, audio_data, num_frames) -> i32`:
  - Reconstruct `&AndroidState` from `userdata` (borrow, **do not** drop).
  - Lock buffer; if `!playing`, zero-fill `audio_data` (f32) and return CONTINUE (keep `read_pos`).
  - Copy `min(num_frames * channels, available)` f32 samples `*volume` into `audio_data`;
    advance `read_pos`; zero-fill any remaining frames (underrun -> silence).
  - Return `AAUDIO_CALLBACK_RESULT_CONTINUE`. Keep callback allocation-free.
- `AndroidAudioOutputStream::play/pause`: set `playing` atomic. `stop`: `requestStop` + `close` + `AAudioStreamBuilder_delete`; in `Drop` do `drop(Arc::from_raw(self.userdata))` to balance the leaked `into_raw`.
- Robustness: after `openStream`, read `AAudioStream_getSampleRate`; if != 48000 log a warning
  (pitch mismatch). If `openStream` fails on FLOAT, optionally retry with `AAUDIO_FORMAT_I16`
  + f32->i16 conversion (out of scope stretch; note in code).

### 2. `src/platform/mod.rs`
- Add `#[cfg(all(target_os = "android", feature = "backend"))] pub mod android;`
- Add `android::init();` in `init()` under same cfg.

### 3. `src/player/engine.rs` (backend selection, ~lines 46-74)
- Add a branch alongside linux/macos/windows:
  `#[cfg(all(target_os = "android", feature = "backend"))] { Box::new(crate::platform::android::AndroidAudioBackend) }`
  Place it before the generic `not(feature = "backend")` Noop fallback so Android+backend wins.

### 4. `src/platform/common.rs`
- `PlatformInfo::current()`: add `#[cfg(target_os = "android")] ("android","Android")`.
- `runtime_dir()`: add explicit `target_os = "android"` branch returning
  `std::env::temp_dir().join("l337-audio-server").join("runtime")` (Termux-friendly).

### 5. `Cargo.toml`
- No new dependency required (raw FFI). Leave `backend` feature as-is.
- Optional: add a comment/doc that `android` backend needs NDK r28b + API 26.

### 6. `.github/workflows/build.yml` (`build-android` job)
- In "Install Android NDK" step, rename `aarch64-linux-android21-clang` -> `aarch64-linux-android26-clang`
  for `CC_aarch64-linux-android`, `CXX_aarch64-linux-android`, `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`
  (lines 116-118).
- In "Build release binary" step (line 120), **remove `--no-default-features`** so default
  `backend` feature is built and the Android AAudio backend compiles.
- (NDK r28b already provides `libaaudio` at API 26; if the linker can't find it, add an explicit
  `-L` to `<ndk>/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/aarch64-linux-android/26`
  via `RUSTFLAGS`/`cargo:rustc-link-search`.)

### 7. `scripts/build.sh` (optional/doc)
- Document `aarch64-linux-android` as a supported cross target for local builds (it currently
  only lists `aarch64-unknown-linux-gnu`).

## Validation
- Kick off Android-only CI run (already verified dispatch works on `testing` branch):
  `gh api repos/tim-projects/l337-audio-server/actions/workflows/341781605/dispatches -X POST --input <(echo '{"ref":"refs/heads/testing","inputs":{"target":"aarch64-linux-android"}}')`
- Confirm `Build - aarch64-linux-android` is **green** (not skipped, not the Noop path) and the
  `l337-audio-server-aarch64-linux-android` artifact is produced.
- CI logs should show the new `Starting AAudio stream ...` line, proving the real backend ran
  (not `NoopAudioBackend`).
- Local: full Android build needs the NDK, so rely on CI. `cargo check` on the host will NOT
  exercise `android.rs` (target-gated); acceptable.
- Device/Termux (manual, post-merge): run the binary, play a track, verify audible output.

## Risks
- Some devices may not expose `AAUDIO_FORMAT_FLOAT` -> add I16 fallback if CI/device shows open failure.
- Sample-rate mismatch -> pitch error; mitigated by requesting 48000 (universally supported).
- AAudio callback runs on a real-time thread: keep it lock-then-copy-then-unlock, no allocations.
- Link failure for `libaaudio` -> add explicit sysroot lib search path (see step 6).

## Open questions / future
- APK-service wrapping (later): binary's self-contained native audio is compatible; add
  audio-focus handling at the APK/service layer or via JNI when needed.
- Hardware volume: `AAudioStream_setVolume` could replace software gain later (enhancement).
