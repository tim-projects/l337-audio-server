# Replace cpal with native audio backends

## Goal
Remove `cpal` entirely. Use native audio APIs per platform: PipeWire on Linux, CoreAudio on macOS, WASAPI on Windows. Each binary only links its own platform backend.

## Current state
- `src/player/engine.rs` uses `cpal` for output stream + config discovery
- `platform/` already isolates OS-specific init
- Dummy mode (`new_dummy`) already bypasses audio

## Changes

### `platform/common.rs`
- Add `AudioBackend` and `AudioOutputStream` traits
- Add `NoopAudioBackend` / `NoopAudioOutputStream` for dummy mode
- Remove any cpal-related helpers if present

### `platform/linux.rs`
- Add `pipewire = "0.10"`
- Implement `PipeWireAudioBackend`
- Create stream with `MEDIA_TYPE = "Audio"`, `MEDIA_CATEGORY = "Playback"`, `MEDIA_ROLE = "Music"`
- Set node name `l337-audio-server` in stream properties
- Use `StreamFlags::AUTOCONNECT | MAP_BUFFERS | RT_PROCESS`
- Run `MainLoop` on a dedicated thread; bridge audio via channel to engine callback
- Remove env var hacks (`PIPEWIRE_RUNTIME_DIR`, `ALSA_CLIENT_NAME`, `PULSE_PROP_APPLICATION_NAME`)

### `platform/macos.rs`
- Add `coreaudio-rs = "0.10"`
- Implement `CoreAudioAudioBackend`
- Use `coreaudio::AudioUnit` render callback
- Default 48 kHz / stereo

### `platform/windows.rs`
- Add `wasapi = "0.10"`
- Implement `WasapiAudioBackend`
- Use render-mode audio client with callback
- Default 48 kHz / stereo

### `src/player/engine.rs`
- Remove `cpal` imports and `cpal::Stream` field
- Replace `Option<cpal::Stream>` with `Option<Box<dyn AudioOutputStream>>`
- Select backend at startup via `#[cfg(target_os = "...")]`
- Call `backend.start_stream(name, sample_rate, channels, callback)` instead of cpal
- Preserve existing `audio_buffer` + `volume` logic inside callback

### `src/api/handlers.rs`
- Update or remove cpal-specific comment if present

### `Cargo.toml`
- Remove `cpal = "0.15"`
- Add target-specific dependencies:
  - `[target.'cfg(target_os = "linux")'.dependencies] pipewire = "0.10"`
  - `[target.'cfg(target_os = "macos")'.dependencies] coreaudio-rs = "0.10"`
  - `[target.'cfg(target_os = "windows")'.dependencies] wasapi = "0.10"`

## Validation
- `cargo check` on Linux
- `cargo test` passes (dummy mode unaffected)
- `size -a ./bin/l337-audio-server` before/after
- Linux: `pw-cli ls-node` shows `l337-audio-server` with no `alsa_playback.` prefix
- Playback works end-to-end on each platform

## Out of scope
- Input/recording streams
- Sample-rate/channel auto-discovery beyond native defaults
- macOS/Windows platform-specific service integrations beyond audio
