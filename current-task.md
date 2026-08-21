# Replace cpal with native audio backends

## Checklist

### platform/common.rs
- [ ] Move `AudioBuffer` struct from `engine.rs` to `common.rs` and make it public
  - **Finding:** `AudioBuffer` is currently private in `engine.rs` (lines 19-26). It is used extensively there and also needed by audio backends for the callback. Moving it to `common.rs` creates a dependency from `platform` upward? No — `platform` is a lower-level module, `engine.rs` depends on `platform`. So `AudioBuffer` can live in `common.rs` and be imported by `engine.rs`.
  - **Mitigation:** Move struct + impl block to `common.rs`, update imports in `engine.rs`.
- [ ] Add `AudioOutputStream` trait with `play()`, `pause()`, `stop()` methods
  - **Finding:** `engine.rs` currently calls `stream.pause()`, `stream.play()` on `Option<cpal::Stream>`. The new trait must match these method names/signatures.
  - **Decision:** `play(&mut self) -> Result<(), String>`, `pause(&mut self) -> Result<(), String>`, `stop(&mut self)`.
- [ ] Add `AudioBackend` trait with `start_stream(...)` method
  - **Finding:** Current cpal callback captures `ab: Arc<Mutex<AudioBuffer>>` and `vol: Arc<Mutex<f32>>` and locks them per-callback. The new `start_stream` must receive these Arc clones so the backend can capture them in its own callback, preserving the exact locking pattern.
  - **Decision:** Signature: `fn start_stream(&self, name: &str, sample_rate: u32, channels: u16, audio_buffer: Arc<Mutex<AudioBuffer>>, volume: Arc<Mutex<f32>>) -> Result<Box<dyn AudioOutputStream>, String>`
- [ ] Add `NoopAudioBackend` and `NoopAudioOutputStream` for dummy mode
  - **Finding:** `new_dummy()` currently sets `stream: None`, making `audio_available: false` in status. Using `NoopAudioBackend` will set `stream: Some(...)`, changing `audio_available` to `true` in dummy mode.
  - **Mitigation:** Verify no API consumers rely on `audio_available == false` in dummy mode. The plan explicitly states `NoopAudioBackend` should be used for dummy mode tests, so this is intentional.
- [ ] Remove any cpal-related helpers if present
  - **Finding:** `audio_env()` in `common.rs` pushes `XDG_RUNTIME_DIR` and `PIPEWIRE_RUNTIME_DIR` — these were for cpal/PipeWire discovery. Native PipeWire backend won't need them.
  - **Mitigation:** Remove `audio_env()` function and its usages in `linux.rs`, `macos.rs`, `windows.rs`. Keep `runtime_dir()` and `ensure_runtime_dir()` as they may be used by `single_instance.rs`.

### platform/linux.rs
- [ ] Add `pipewire = "0.10"` dependency
  - **Finding:** Added via target-specific dependency in Cargo.toml, not inline.
- [ ] Implement `PipeWireAudioBackend` struct
- [ ] Implement `PipeWireAudioOutputStream` struct
- [ ] Create stream with `MEDIA_TYPE = "Audio"`, `MEDIA_CATEGORY = "Playback"`, `MEDIA_ROLE = "Music"`
- [ ] Set node name `l337-audio-server` in stream properties
  - **Finding:** This allows existing `set_pipewire_sink_input_volume()` in `engine.rs` to find our stream via `pactl list sink-inputs` by matching `node.name = l337-audio-server`. Do NOT remove this volume control path.
- [ ] Use `StreamFlags::AUTOCONNECT | MAP_BUFFERS | RT_PROCESS`
- [ ] Run `MainLoop` on a dedicated thread; bridge audio via channel to engine callback
  - **Finding:** The plan says "bridge via channel" but discovery says "preserve exact cpal pattern". These conflict.
  - **Decision:** The PipeWire process callback runs on the MainLoop thread. To preserve cpal's direct-locking pattern, the callback should directly access `Arc<Mutex<AudioBuffer>>` and `Arc<Mutex<f32>>` — no channel bridge. The MainLoop thread IS the audio thread.
  - **Mitigation:** Implement callback that locks `audio_buffer` and `volume` directly, exactly mirroring lines 154-166 of current `engine.rs`.
- [ ] Remove env var hacks (`PIPEWIRE_RUNTIME_DIR`, `ALSA_CLIENT_NAME`, `PULSE_PROP_APPLICATION_NAME`)
  - **Finding:** `linux.rs::init()` currently sets these at lines 16-33. With native PipeWire, they are unnecessary. The `init()` function can be simplified to just `ensure_runtime_dir()`.
- [ ] Remove `check_pipewire_available()` and `start_pipewire_service()` if no longer needed
  - **Finding:** `check_pipewire_available()` spawns `Command::new("pipewire")` which may not exist (discovery note). `engine.rs` line 113 calls it.
  - **Mitigation:** Remove the function and its call in `engine.rs`. Let the PipeWire backend fail naturally with a descriptive error if PipeWire is unavailable. Keep `start_pipewire_service()` if it's used elsewhere, otherwise remove it too. **Check:** `start_pipewire_service()` is only defined, not called anywhere else in the codebase — safe to remove.

### platform/macos.rs
- [ ] Add `coreaudio-rs = "0.10"` dependency
  - **Finding:** Added via target-specific dependency in Cargo.toml.
- [ ] Implement `CoreAudioAudioBackend` struct
- [ ] Implement `CoreAudioAudioOutputStream` struct
- [ ] Use `coreaudio::AudioUnit` render callback
- [ ] Default 48 kHz / stereo

### platform/windows.rs
- [ ] Add `wasapi = "0.10"` dependency
  - **Finding:** Added via target-specific dependency in Cargo.toml.
- [ ] Implement `WasapiAudioBackend` struct
- [ ] Implement `WasapiAudioOutputStream` struct
- [ ] Use render-mode audio client with callback
- [ ] Default 48 kHz / stereo
  - **Finding:** `wasapi` requires COM initialization (`CoInitializeEx`) per thread. Must call before any WASAPI APIs.

### src/player/engine.rs
- [ ] Remove `cpal` imports
  - **Finding:** `use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};` at line 3.
- [ ] Remove `cpal::Stream` field from `PlayerEngine`
  - **Finding:** `stream: Option<cpal::Stream>` at line 48.
- [ ] Replace `Option<cpal::Stream>` with `Option<Box<dyn AudioOutputStream>>`
  - **Finding:** Current `unsafe impl Send/Sync` at lines 67-68 exists because `cpal::Stream` is `!Send`. With `Box<dyn AudioOutputStream>` where `AudioOutputStream: Send + Sync`, the box is naturally `Send + Sync`.
  - **Mitigation:** Remove `unsafe impl Send for PlayerEngine` and `unsafe impl Sync for PlayerEngine` entirely.
- [ ] Remove `unsafe impl Send for PlayerEngine` and `unsafe impl Sync for PlayerEngine`
- [ ] Select backend at startup via `#[cfg(target_os = "...")]`
  - **Finding:** `new()` currently calls `init_audio_device()` which does cpal setup. Need to replace with platform backend dispatch.
  - **Decision:** Use `cfg` in `new()` to instantiate the correct backend, or add a `backend()` function in `platform/mod.rs` returning `&'static dyn AudioBackend`.
- [ ] Call `backend.start_stream(name, sample_rate, channels, callback)` instead of cpal
  - **Finding:** Current `init_audio_device()` returns `(Option<cpal::Stream>, u32)` with discovered sample rate. Native backends will use default 48 kHz.
  - **Mitigation:** Hardcode 48000 Hz / stereo as defaults (matches macOS/Windows plan). Remove sample-rate discovery logic.
- [ ] Preserve existing `audio_buffer` + `volume` logic inside callback
  - **Finding:** The callback at lines 148-166 locks `ab` and `vol`, copies PCM with volume, zeros remainder. This exact logic must be preserved in each backend's audio callback.
  - **Mitigation:** In each backend's process/render callback, replicate: `let mut buf = ab.lock().unwrap(); let available = ...; let to_copy = ...; let vol = *vol.lock().unwrap(); for loop; buf.read_pos += to_copy; zero remainder`.
- [ ] Update `pause()` / `resume()` / `stop()` to use new stream interface
  - **Finding:** `pause()` (line 347) and `resume()` (line 356) currently call `stream.pause()` / `stream.play()`. `stop()` (line 365) does not call stream methods — it clears buffer and state.
  - **Mitigation:** Update `pause()`/`resume()` to use `&mut self.stream` (need `as_mut()` since `Box<dyn AudioOutputStream>` is not `Copy`). `stop()` stays the same but could also call `stream.stop()` if needed.

### src/api/handlers.rs
- [ ] Update or remove cpal-specific comment at line 16
  - **Finding:** Lines 14-17 say: "tokio::sync::Mutex<PlayerEngine> is Send+Sync because PlayerEngine has an explicit unsafe impl for Send+Sync (cpal::Stream is conservatively !Send even though the underlying OS handle is safe to move between threads)."
  - **Mitigation:** Update to: "tokio::sync::Mutex<PlayerEngine> is Send+Sync because PlayerEngine is naturally Send+Sync (all fields, including the boxed audio stream handle, implement Send+Sync)."

### Cargo.toml
- [ ] Remove `cpal = "0.15"`
  - **Finding:** Line 11. Must be removed to avoid linking unused cpal code.
- [ ] Add `[target.'cfg(target_os = "linux")'.dependencies] pipewire = "0.10"`
- [ ] Add `[target.'cfg(target_os = "macos")'.dependencies] coreaudio-rs = "0.10"`
- [ ] Add `[target.'cfg(target_os = "windows")'.dependencies] wasapi = "0.10"`
  - **Finding:** Target-specific dependencies automatically create per-platform binaries. No workspace split needed.

### Validation
- [ ] `cargo check` on Linux
  - **Mitigation:** Use `CARGO_HOME=/tmp/cargo-home cargo check` per AGENTS.md to avoid writing to `./target`.
- [ ] `cargo test` passes (dummy mode unaffected)
  - **Finding:** `tests.rs` uses `PlayerEngine::new_dummy()`. Since `NoopAudioBackend` has no external deps, tests should pass.
- [ ] `size -a ./bin/l337-audio-server` before/after
  - **Finding:** AGENTS.md says use `cargo bloat --release --crates` for comparison, not just `size`.
  - **Mitigation:** Use `./scripts/build.sh` then `cargo bloat --release --crates`.
- [ ] Linux: `pw-cli ls-node` shows `l337-audio-server` with no `alsa_playback.` prefix
  - **Finding:** With native PipeWire stream properties (`node.name = "l337-audio-server"`), the node should appear with our name directly.
- [ ] Playback works end-to-end on each platform

### Additional Findings
- `streaming_decode_sync()` at line 967 has a comment mentioning "for the cpal callback to consume" — should be updated to "for the audio callback to consume".
- `set_pipewire_sink_input_volume()` in `engine.rs` uses `pactl` to find sink input by `node.name = l337-audio-server`. Keeping the node name in PipeWire properties ensures this continues to work.
- `main.rs` calls `platform::init()` before engine creation. `linux.rs::init()` currently sets env vars that cpal needed. After removal, `init()` can be simplified.
- `single_instance.rs` uses `runtime_dir()` — unaffected by audio changes.
- The `libloading` crate is already in `Cargo.toml`. Could be used for a `dlopen`-based `check_pipewire_available()` instead of spawning `pipewire --version`, but the plan says to remove the check entirely.
- The `tokio` runtime in `main.rs` is single-threaded (`#[tokio::main]`). PipeWire's `MainLoop` will run on its own thread, so no conflict.
- `PlayerEngine` is wrapped in `tokio::sync::Mutex` in `handlers.rs`. Since `PlayerEngine` will be naturally `Send + Sync`, this wrapper remains valid.

### Risks & Mitigations
| Risk | Mitigation |
|------|-----------|
| `pipewire` crate v0.10 API differs from assumptions | Implement, run `cargo check`, fix API errors iteratively |
| `coreaudio-rs` / `wasapi` not available on Linux build | Target-specific deps ensure they only compile on their platform |
| `AudioBuffer` move breaks tests | `tests.rs` only uses `new_dummy()` which won't touch audio; run `cargo test` after move |
| `audio_available` semantics change in dummy mode | Verify no handlers depend on `audio_available == false` in dummy mode |
| PipeWire MainLoop thread leak on drop | Implement `Drop` for `PipeWireAudioOutputStream` to signal MainLoop exit |
| Volume control via `pactl` breaks without env vars | Keep `node.name` property; `pactl` matches by node name, not env vars |

---

## Agent Implementation Prompt

You are implementing the plan documented above and in `.kilo/plans/1786963621222-native-audio-backends.md`. Treat the plan as the source of truth. Follow the checklist and findings above. Do not deviate from the documented decisions and mitigations.

### Overall Goal
Remove `cpal` entirely. Replace it with native audio backends per platform:
- Linux: PipeWire via `pipewire` crate v0.10
- macOS: CoreAudio via `coreaudio-rs` crate v0.10
- Windows: WASAPI via `wasapi` crate v0.10

The `NoopAudioBackend` is used for `--dummy` mode on all platforms.

### Step 1: Update `Cargo.toml`
1. Remove `cpal = "0.15"` from `[dependencies]`.
2. Add three target-specific dependency sections:
   ```toml
   [target.'cfg(target_os = "linux")'.dependencies]
   pipewire = "0.10"

   [target.'cfg(target_os = "macos")'.dependencies]
   coreaudio-rs = "0.10"

   [target.'cfg(target_os = "windows")'.dependencies]
   wasapi = "0.10"
   ```
3. Do NOT add any other new dependencies.

### Step 2: Update `src/platform/common.rs`
1. Move `AudioBuffer` from `engine.rs` to `common.rs` and make it `pub`.
   - Copy the struct definition and its `new()` method exactly as-is from `engine.rs` lines 19-39.
   - Remove `AudioBuffer` from `engine.rs` after moving.
2. Add the `AudioOutputStream` trait:
   ```rust
   pub trait AudioOutputStream: Send + Sync {
       fn play(&mut self) -> Result<(), String>;
       fn pause(&mut self) -> Result<(), String>;
       fn stop(&mut self);
   }
   ```
3. Add the `AudioBackend` trait:
   ```rust
   pub trait AudioBackend: Send + Sync {
       fn start_stream(
           &self,
           name: &str,
           sample_rate: u32,
           channels: u16,
           audio_buffer: std::sync::Arc<std::sync::Mutex<AudioBuffer>>,
           volume: std::sync::Arc<std::sync::Mutex<f32>>,
       ) -> Result<Box<dyn AudioOutputStream>, String>;
   }
   ```
4. Add `NoopAudioBackend` and `NoopAudioOutputStream`:
   ```rust
   pub struct NoopAudioBackend;
   pub struct NoopAudioOutputStream;

   impl AudioBackend for NoopAudioBackend {
       fn start_stream(
           &self,
           _name: &str,
           _sample_rate: u32,
           _channels: u16,
           _audio_buffer: std::sync::Arc<std::sync::Mutex<AudioBuffer>>,
           _volume: std::sync::Arc<std::sync::Mutex<f32>>,
       ) -> Result<Box<dyn AudioOutputStream>, String> {
           Ok(Box::new(NoopAudioOutputStream))
       }
   }

   impl AudioOutputStream for NoopAudioOutputStream {
       fn play(&mut self) -> Result<(), String> { Ok(()) }
       fn pause(&mut self) -> Result<(), String> { Ok(()) }
       fn stop(&mut self) {}
   }
   ```
5. Remove `audio_env()` function and all its usages. Remove the `Vec<...>` return type. Keep `runtime_dir()` and `ensure_runtime_dir()` unchanged.
6. Update tests: remove any test that calls `audio_env()`.

### Step 3: Update `src/platform/linux.rs`
1. Remove the `audio_env` import.
2. Simplify `init()` to only call `ensure_runtime_dir()`. Remove all `std::env::set_var(...)` calls.
3. Remove `check_pipewire_available()` entirely.
4. Remove `start_pipewire_service()` entirely (it is not called anywhere else).
5. Implement `PipeWireAudioBackend` and `PipeWireAudioOutputStream`:
   ```rust
   use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer, runtime_dir};
   use std::sync::atomic::{AtomicBool, Ordering};
   use std::sync::Arc;
   use std::sync::Mutex;

   pub struct PipeWireAudioBackend;

   pub struct PipeWireAudioOutputStream {
       playing: Arc<AtomicBool>,
       main_loop: Option<pipewire::MainLoop>,
   }

   impl AudioBackend for PipeWireAudioBackend {
       fn start_stream(
           &self,
           name: &str,
           sample_rate: u32,
           channels: u16,
           audio_buffer: Arc<Mutex<AudioBuffer>>,
           volume: Arc<Mutex<f32>>,
       ) -> Result<Box<dyn AudioOutputStream>, String> {
           unsafe { pipewire::init() };

           let main_loop = pipewire::MainLoop::new(None)
               .map_err(|e| format!("Failed to create PipeWire main loop: {}", e))?;

           let context = pipewire::Context::new(&main_loop)
               .map_err(|e| format!("Failed to create PipeWire context: {}", e))?;

           let core = context.connect(None, None, None)
               .map_err(|e| format!("Failed to connect to PipeWire: {}", e))?;

           let playing = Arc::new(AtomicBool::new(true));

           let ab = audio_buffer.clone();
           let vol = volume.clone();
           let playing_cb = playing.clone();

           let stream = pipewire::Stream::new(
               &core,
               name,
               pipewire::properties::Properties::new()
                   .insert("media.type", "Audio")
                   .insert("media.category", "Playback")
                   .insert("media.role", "Music")
                   .insert("node.name", name)
                   .insert("node.description", "L337 Audio Server"),
           )
           .map_err(|e| format!("Failed to create PipeWire stream: {}", e))?;

           let stream_flags = pipewire::StreamFlags::AUTOCONNECT
               | pipewire::StreamFlags::MAP_BUFFERS
               | pipewire::StreamFlags::RT_PROCESS;

           stream.connect(
               stream_flags,
               Some(pipewire::spa::SpaTypes::OBJECT_AUDIO_OUTPUT),
               Some("playback"),
               Some(pipewire::spa::param::AudioFormat::F32),
               Some(&[
                   pipewire::spa::param::AudioInfoProperty::SampleRate(sample_rate),
                   pipewire::spa::param::AudioInfoProperty::Channels(channels),
               ]),
           )
           .map_err(|e| format!("Failed to connect PipeWire stream: {}", e))?;

           stream.set_process(move |stream| {
               let playing = playing_cb.load(Ordering::SeqCst);
               if !playing {
                   return;
               }

               let mut buf = ab.lock().unwrap();
               let available = buf.pcm.len().saturating_sub(buf.read_pos);
               let vol = *vol.lock().unwrap();

               if let Ok(Some(mut data)) = stream.dequeue_buffer() {
                   if let Some(dst) = data.data_mut() {
                       let dst = dst.as_slice_mut();
                       if dst.is_empty() {
                           return;
                       }

                       let to_copy = dst.len().min(available);
                       for (d, s) in dst.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                           *d = s * vol;
                       }
                       buf.read_pos += to_copy;

                       for sample in &mut dst[to_copy..] {
                           *sample = 0.0;
                       }
                   }
               }
           });

           stream.set_active(true)
               .map_err(|e| format!("Failed to activate PipeWire stream: {}", e))?;

           let main_loop_clone = main_loop.clone();
           std::thread::spawn(move || {
               main_loop_clone.run();
           });

           Ok(Box::new(PipeWireAudioOutputStream {
               playing,
               main_loop: Some(main_loop),
           }))
       }
   }

   impl AudioOutputStream for PipeWireAudioOutputStream {
       fn play(&mut self) -> Result<(), String> {
           self.playing.store(true, Ordering::SeqCst);
           Ok(())
       }

       fn pause(&mut self) -> Result<(), String> {
           self.playing.store(false, Ordering::SeqCst);
           Ok(())
       }

       fn stop(&mut self) {
           self.playing.store(false, Ordering::SeqCst);
           if let Some(ml) = self.main_loop.take() {
               ml.quit();
           }
       }
   }
   ```
   **Note:** The exact `pipewire` crate API may differ from the above. After writing the initial version, run `cargo check` and fix any API mismatches (method names, types, builder patterns) until it compiles. The key invariants are:
   - `pipewire::init()` must be called before any other pipewire functions.
   - `MainLoop` must run on its own thread.
   - The process callback must directly lock `audio_buffer` and `volume` exactly like the old cpal callback.
   - Stream properties must include `node.name = "l337-audio-server"`.
   - `play()` / `pause()` toggle a shared flag checked by the callback; `stop()` quits the main loop.

### Step 4: Update `src/platform/macos.rs`
1. Remove the `audio_env` import.
2. Simplify `init()` to just call `runtime_dir()` for side effects if needed, or make it a no-op.
3. Implement `CoreAudioAudioBackend` and `CoreAudioAudioOutputStream`:
   ```rust
   use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};
   use std::sync::Arc;
   use std::sync::Mutex;

   pub struct CoreAudioAudioBackend;
   pub struct CoreAudioAudioOutputStream {
       _audio_unit: coreaudio::audio_unit::AudioUnit,
   }

   impl AudioBackend for CoreAudioAudioBackend {
       fn start_stream(
           &self,
           _name: &str,
           sample_rate: u32,
           channels: u16,
           audio_buffer: Arc<Mutex<AudioBuffer>>,
           volume: Arc<Mutex<f32>>,
       ) -> Result<Box<dyn AudioOutputStream>, String> {
           let mut audio_unit = coreaudio::audio_unit::AudioUnit::new(
               coreaudio::audio_unit::AudioUnitType::Output,
           )
           .map_err(|e| format!("Failed to create AudioUnit: {}", e))?;

           let ab = audio_buffer.clone();
           let vol = volume.clone();

           audio_unit
               .set_render_callback(move |data: &mut [f32], _| {
                   let mut buf = ab.lock().unwrap();
                   let available = buf.pcm.len().saturating_sub(buf.read_pos);
                   let vol = *vol.lock().unwrap();

                   let to_copy = data.len().min(available);
                   for (d, s) in data.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                       *d = s * vol;
                   }
                   buf.read_pos += to_copy;

                   for sample in &mut data[to_copy..] {
                       *sample = 0.0;
                   }

                   Ok(())
               })
               .map_err(|e| format!("Failed to set render callback: {}", e))?;

           let stream_format = coreaudio::audio_unit::StreamFormat::new()
               .with_sample_rate(sample_rate as f64)
               .with_channels(channels as usize);

           audio_unit
               .set_stream_format(&stream_format)
               .map_err(|e| format!("Failed to set stream format: {}", e))?;

           audio_unit
               .start()
               .map_err(|e| format!("Failed to start AudioUnit: {}", e))?;

           Ok(Box::new(CoreAudioAudioOutputStream {
               _audio_unit: audio_unit,
           }))
       }
   }

   impl AudioOutputStream for CoreAudioAudioOutputStream {
       fn play(&mut self) -> Result<(), String> { Ok(()) }
       fn pause(&mut self) -> Result<(), String> { Ok(()) }
       fn stop(&mut self) {}
   }
   ```
   **Note:** The exact `coreaudio-rs` API may differ. Run `cargo check` and fix API mismatches. The key invariant is the callback must replicate the cpal buffer-filling logic.

### Step 5: Update `src/platform/windows.rs`
1. Remove the `audio_env` import.
2. Simplify `init()` to a no-op.
3. Implement `WasapiAudioBackend` and `WasapiAudioOutputStream`:
   ```rust
   use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};
   use std::sync::Arc;
   use std::sync::Mutex;

   pub struct WasapiAudioBackend;
   pub struct WasapiAudioOutputStream {
       _audio_client: wasapi::AudioClient,
       _render_client: wasapi::RenderClient,
   }

   impl AudioBackend for WasapiAudioBackend {
       fn start_stream(
           &self,
           _name: &str,
           sample_rate: u32,
           channels: u16,
           audio_buffer: Arc<Mutex<AudioBuffer>>,
           volume: Arc<Mutex<f32>>,
       ) -> Result<Box<dyn AudioOutputStream>, String> {
           unsafe { ole32::CoInitializeEx(None, ole32::COINIT_MULTITHREADED) };

           let collection = wasapi::DeviceCollection::new(&wasapi::DEVICE_STATE_ACTIVE)
               .map_err(|e| format!("Failed to enumerate devices: {}", e))?;

           let device = collection
               .get_default(wasapi::DEVICE_ROLE_CONSOLE, wasapi::DATAFLOW_RENDER)
               .map_err(|e| format!("Failed to get default render device: {}", e))?;

           let audio_client = device
               .get_iaudioclient()
               .map_err(|e| format!("Failed to get AudioClient: {}", e))?;

           let mix_format = audio_client
               .get_mix_format()
               .map_err(|e| format!("Failed to get mix format: {}", e))?;

           let buffer_frames = (sample_rate as u32 / 10) as u32;

           audio_client
               .initialize(
                   wasapi::AUDCLNT_SHAREMODE_SHARED,
                   wasapi::AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                   buffer_frames * mix_format.get_block_align() as u32,
                   0,
                   &mix_format,
                   None,
               )
               .map_err(|e| format!("Failed to initialize AudioClient: {}", e))?;

           let render_client = audio_client
               .get_service()
               .map_err(|e| format!("Failed to get RenderClient: {}", e))?;

           let event = unsafe {
               winapi::um::synchapi::CreateEventW(
                   std::ptr::null_mut(),
                   0,
                   0,
                   std::ptr::null(),
               )
           };

           if event.is_null() {
               return Err("Failed to create event handle".to_string());
           }

           audio_client
               .set_event_handle(event)
               .map_err(|e| format!("Failed to set event handle: {}", e))?;

           let ab = audio_buffer.clone();
           let vol = volume.clone();

           std::thread::spawn(move || {
               loop {
                   unsafe { winapi::um::synchapi::WaitForSingleObject(event, winapi::um::winbase::INFINITE) };

                   let mut buffer = match render_client.get_buffer(buffer_frames as u32) {
                       Ok(b) => b,
                       Err(_) => break,
                   };

                   let data = buffer.data_mut();
                   if data.is_empty() {
                       let _ = render_client.release_buffer(buffer_frames as u32, 0);
                       break;
                   }

                   let mut buf = ab.lock().unwrap();
                   let available = buf.pcm.len().saturating_sub(buf.read_pos);
                   let v = *vol.lock().unwrap();

                   let to_copy = data.len().min(available);
                   for (d, s) in data.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                       *d = s * v;
                   }
                   buf.read_pos += to_copy;

                   for sample in &mut data[to_copy..] {
                       *sample = 0.0;
                   }

                   let _ = render_client.release_buffer(buffer_frames as u32, 0);
               }
           });

           audio_client
               .start()
               .map_err(|e| format!("Failed to start AudioClient: {}", e))?;

           Ok(Box::new(WasapiAudioOutputStream {
               _audio_client: audio_client,
               _render_client: render_client,
           }))
       }
   }

   impl AudioOutputStream for WasapiAudioOutputStream {
       fn play(&mut self) -> Result<(), String> { Ok(()) }
       fn pause(&mut self) -> Result<(), String> { Ok(()) }
       fn stop(&mut self) {}
   }
   ```
   **Note:** The exact `wasapi` crate + Windows API bindings may differ. Run `cargo check` on a Windows target (or cross-check API names) and fix mismatches. The key invariants are:
   - `CoInitializeEx` must be called before WASAPI APIs.
   - Use shared mode with event-driven callback.
   - The background thread must lock `audio_buffer` and `volume` exactly like the old cpal callback.
   - Default to 48 kHz / stereo.

### Step 6: Update `src/player/engine.rs`
1. Remove `use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};` (line 3).
2. Add imports:
   ```rust
   use crate::platform::common::{AudioBuffer, AudioBackend, AudioOutputStream};
   use std::sync::atomic::{AtomicBool, Ordering};
   ```
3. In `PlayerEngine` struct (lines 47-61):
   - Replace `stream: Option<cpal::Stream>` with `stream: Option<Box<dyn AudioOutputStream>>`
4. Remove the `unsafe impl Send for PlayerEngine {}` and `unsafe impl Sync for PlayerEngine {}` blocks (lines 67-68).
5. Rewrite `new()` (lines 71-92):
   - Remove the call to `init_audio_device()`.
   - Create `audio_buffer` with `AudioBuffer::new(48000, 0)`.
   - Create `volume = Arc::new(Mutex::new(1.0))`.
   - Instantiate the platform backend using `#[cfg(target_os = "...")]`:
     ```rust
     let backend: Box<dyn AudioBackend> = {
         #[cfg(target_os = "linux")]
         { Box::new(crate::platform::linux::PipeWireAudioBackend) }
         #[cfg(target_os = "macos")]
         { Box::new(crate::platform::macos::CoreAudioAudioBackend) }
         #[cfg(target_os = "windows")]
         { Box::new(crate::platform::windows::WasapiAudioBackend) }
         #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
         { Box::new(crate::platform::common::NoopAudioBackend) }
     };
     ```
   - Call `backend.start_stream("l337-audio-server", 48000, 2, audio_buffer.clone(), volume.clone())?`.
   - Store `stream: Some(stream)` in the struct.
6. Rewrite `new_dummy()` (lines 94-110):
   - Replace `stream: None` with `stream: Some(Box::new(crate::platform::common::NoopAudioOutputStream))`.
   - Keep `audio_buffer: Arc::new(Mutex::new(AudioBuffer::new(48000, 0)))` and `volume: Arc::new(Mutex::new(1.0)))`.
7. Delete the entire `init_audio_device()` function (lines 112-185).
8. Update `pause()` (lines 347-354):
   ```rust
   pub fn pause(&mut self) {
       if let Some(stream) = self.stream.as_mut() {
           if let Err(e) = stream.pause() {
               error!("pause failed: {}", e);
           }
       }
       self.state = PlayerStateLabel::Paused;
   }
   ```
9. Update `resume()` (lines 356-363):
   ```rust
   pub fn resume(&mut self) {
       if let Some(stream) = self.stream.as_mut() {
           if let Err(e) = stream.resume() {
               error!("resume failed: {}", e);
           }
       }
       self.state = PlayerStateLabel::Playing;
   }
   ```
   **Note:** `AudioOutputStream` defines `play()`, not `resume()`. Either rename the trait method to `resume()` or change `engine.rs` to call `play()`. **Decision:** Keep trait method as `play()` (matches cpal API) and call `stream.play()` from `resume()`. Update the trait definition in `common.rs` accordingly if needed.
10. Update `stop()` (lines 365-379): no stream method call needed currently, but you may add `stream.stop()` if the trait supports it. For now, leave the body unchanged.
11. Update `streaming_decode_sync()` comment (line 967): change "for the cpal callback to consume" to "for the audio callback to consume".

### Step 7: Update `src/api/handlers.rs`
1. Update the comment at lines 14-17:
   ```rust
   // Use a simple wrapper to make PlayerEngine safely shareable across tasks.
   // tokio::sync::Mutex<PlayerEngine> is Send+Sync because PlayerEngine and all
   // its fields (including the boxed audio stream handle) are naturally Send+Sync.
   ```

### Step 8: Update `src/platform/macos.rs` and `src/platform/windows.rs` init functions
- Remove `audio_env` import and usage.
- Keep `init()` minimal (just `runtime_dir()` call or empty).

### Step 9: Run Validation
1. Run `CARGO_HOME=/tmp/cargo-home cargo check` on Linux. Fix all compilation errors.
2. Run `cargo test`. Ensure all tests pass (especially `test_player_engine_state` which uses `new_dummy()`).
3. Build with `./scripts/build.sh` and compare binary size with `cargo bloat --release --crates`.
4. Verify `pw-cli ls-node` shows `l337-audio-server` after running the server on a PipeWire system.

### Step 10: Real Hardware Validation (this machine has PipeWire)
This machine has a soundcard with PipeWire. Do NOT use `--dummy`. Validate actual playback end-to-end.

#### 10.1 Build the binary
```bash
./scripts/build.sh
```
Expected: `bin/l337-audio-server` exists and is executable.

#### 10.2 Start server over HTTP
```bash
L337__SERVER__PORT=1337 ./bin/l337-audio-server
```
- Confirm it binds to `0.0.0.0:1337` or `127.0.0.1:1337` per config.
- Confirm `/health` returns JSON with `yt_dlp` capability.
- Confirm `/setup` returns a token.

#### 10.3 Play a local audio file over HTTP
```bash
# In another terminal, with the token from /setup:
TOKEN="<token>"
curl -k -X POST https://127.0.0.1:1337/player/play \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"track_id":"local-1","stream_url":"file:///path/to/test.wav","title":"Test"}'
```
- Confirm response is `200` with `{"ok": true, ...}`.
- Confirm `/player/status` returns `"state":"playing"`.
- Confirm audio is audible on the soundcard.

#### 10.4 Play a YouTube URL over HTTP
```bash
curl -k -X POST https://127.0.0.1:1337/player/play \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"track_id":"yt-1","stream_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","title":"YouTube Test"}'
```
- Confirm response is `200`.
- Confirm `/player/status` transitions to `playing`.
- Confirm audio is audible on the soundcard.
- Check server logs for `yt-dlp` download and decode progress.

#### 10.5 Start server over Unix socket
```bash
./bin/l337-audio-server --transport=socket
```
- Confirm socket is created at `~/.cache/l337/l337-audio-server/l337.sock` (or configured path).
- Confirm `curl --unix-socket` can reach `/health` and `/setup`.

#### 10.6 Play a local audio file over Unix socket
```bash
curl --unix-socket ~/.cache/l337/l337-audio-server/l337.sock \
  -k -X POST https://localhost/player/play \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"track_id":"socket-1","stream_url":"file:///path/to/test.wav","title":"Socket Test"}'
```
- Confirm response is `200`.
- Confirm `/player/status` returns `playing`.
- Confirm audio is audible on the soundcard.

#### 10.7 Verify PipeWire sink input
```bash
pactl list sink-inputs
```
- Confirm an entry with `application.name = "l337-audio-server"` or `node.name = "l337-audio-server"` exists while playing.
- Confirm `pactl set-sink-input-volume <index> 50%` works (hardware volume control).

#### 10.8 Pause / Resume / Stop
```bash
curl -k -X POST https://127.0.0.1:1337/player/pause -H "Authorization: Bearer $TOKEN"
curl -k -X POST https://127.0.0.1:1337/player/pause -H "Authorization: Bearer $TOKEN"  # resume
curl -k -X POST https://127.0.0.1:1337/player/pause -H "Authorization: Bearer $TOKEN"  # pause again
```
- Confirm state toggles between `paused` and `playing`.
- Confirm audio output mutes on pause and resumes on play.

#### 10.9 Seek
```bash
curl -k -X POST https://127.0.0.1:1337/player/seek \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"position": 30}'
```
- Confirm response is `200`.
- Confirm `/player/status` shows updated `position_sec`.

#### 10.10 Stop
```bash
curl -k -X POST https://127.0.0.1:1337/player/pause -H "Authorization: Bearer $TOKEN"
```
- Confirm state returns to `stopped`.
- Confirm `pactl list sink-inputs` no longer shows `l337-audio-server`.

#### 10.11 Upload stream playback
```bash
curl -k -X POST https://127.0.0.1:1337/player/play/stream \
  -H "Authorization: Bearer $TOKEN" \
  -H "X-Track-Id: uploaded-1" \
  -H "X-Title: Uploaded" \
  -H "Content-Type: audio/wav" \
  --data-binary @/path/to/test.wav
```
- Confirm response is `200`.
- Confirm `/player/status` returns `playing`.
- Confirm audio is audible on the soundcard.

### Critical Invariants (do not violate)
- The audio callback in every backend must directly lock `Arc<Mutex<AudioBuffer>>` and `Arc<Mutex<f32>>`, copying PCM with volume and zeroing the remainder — exactly mirroring old `engine.rs` lines 154-166.
- `AudioBuffer` lives in `platform/common.rs`, not `engine.rs`.
- `PlayerEngine` must NOT have `unsafe impl Send/Sync` — it is naturally `Send + Sync`.
- `NoopAudioBackend` is used for `--dummy` mode; `stream` is `Some(...)` not `None`.
- PipeWire stream properties must include `node.name = "l337-audio-server"` so existing `set_pipewire_sink_input_volume()` in `engine.rs` continues to work.
- Do NOT remove `set_pipewire_sink_input_volume()` or `set_volume()` from `engine.rs`.
- Do NOT touch `storage`, `decode`, `streaming`, or any non-audio code in `engine.rs`.

### Output
After completing all steps, report back:
1. Which files were modified and a brief summary of changes per file.
2. `cargo check` result.
3. `cargo test` result.
4. Any compilation warnings that were intentionally suppressed or need follow-up.
5. Any API assumptions that turned out to be incorrect and how you resolved them.
