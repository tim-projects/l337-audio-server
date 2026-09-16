# Plan: macOS + Windows Audio Backend Parity with Linux

## Goal

Bring the macOS (CoreAudio) and Windows (WASAPI) audio backends to **feature parity** with Linux (PipeWire) so that:
- Audio only starts when the client sends a `play` command
- `pause` actually suspends audio output
- `stop` terminates the stream and flushes the buffer
- The installer-printed token is the same token the binary uses at runtime
- Runtime directories are created with correct permissions before the lock is acquired

## Current Parity Matrix

| Feature | Linux (PipeWire) | macOS (CoreAudio) | Windows (WASAPI) |
|---|---|---|---|
| `play()` starts audio | ✅ atomic flag + stream active | ❌ no-op | ❌ no-op |
| `pause()` suspends audio | ✅ atomic flag cleared | ❌ no-op | ❌ no-op |
| `stop()` terminates stream | ✅ drops stream, clears PCM | ❌ no-op | ❌ no-op |
| `init()` creates runtime dir | ✅ `ensure_runtime_dir()` | ❌ dir discarded | ❌ no-op |
| Installer token == runtime token | ✅ `STATE_DIRECTORY` or `/var/cache` | ❌ installer writes `~/Library/Application Support`, binary reads `/etc` or `~/.cache` | ❌ installer writes `C:\ProgramData\l337`, binary reads `/etc` or CWD |
| Config readable by binary | ✅ `/etc/l337-audio-server/server.ini` | ❌ installer writes to `~/Library/Application Support`, binary never reads it | ❌ installer writes to `C:\ProgramData`, binary never reads it |

## Root Causes

### 1. Audio control no-ops on macOS and Windows

**macOS** (`src/platform/macos.rs`):
```rust
impl AudioOutputStream for CoreAudioAudioOutputStream {
    fn play(&mut self) -> Result<(), String> { Ok(()) }
    fn pause(&mut self) -> Result<(), String> { Ok(()) }
    fn stop(&mut self) {}
}
```

The `AudioUnit` stream is started inside `start_stream()` before the `AudioOutputStream` is returned. There is no mechanism to pause or stop it later. The render callback runs continuously regardless of play state.

**Windows** (`src/platform/windows.rs`):
```rust
impl AudioOutputStream for WasapiAudioOutputStream {
    fn play(&mut self) -> Result<(), String> { Ok(()) }
    fn pause(&mut self) -> Result<(), String> { Ok(()) }
    fn stop(&mut self) {}
}
```

Same pattern — the WASAPI render thread runs in a loop forever, ignoring play state.

**Linux** (`src/platform/linux.rs`):
```rust
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
    self._stream = None;
}
```

Linux uses an `Arc<AtomicBool>` (`playing`) that the render callback checks. When `false`, the callback returns early — no audio is written to the buffer.

### 2. macOS config path mismatch

The macOS installer (`install.sh:643`) writes config to:
```
~/Library/Application Support/l337-audio-server/server.ini
```

But `load_settings()` in `main.rs:102-119` only reads:
1. `/etc/l337-audio-server/config`
2. `/etc/l337-audio-server/server.ini`
3. `./server.ini` (CWD — launchd runs from `/` or `$HOME`)
4. `L337__*` env vars

So the installer-printed token is orphaned. The binary falls through to `load_or_create_token()`, which on macOS (no `STATE_DIRECTORY`) writes to `~/.cache/l337/l337-audio-server/server_token.txt` — a different path and a different token.

### 3. `init()` doesn't create the runtime dir on macOS

`src/platform/macos.rs:72-74`:
```rust
pub fn init() {
    let _dir = runtime_dir();
}
```

The value is computed and thrown away. The `~/Library/Caches/l337-audio-server/runtime` directory is never created, so the instance lock file can't be created there. The single-instance code in `src/platform/single_instance.rs:166-173` tries to use this path as a lock candidate but silently falls back to `/tmp/l337-audio-server-runtime/instance.lock`.

### 4. Launchd plist lacks `WorkingDirectory`

`install.sh:697-730` — the plist has no `<key>WorkingDirectory</key>`. The binary's CWD when launched by launchd is not guaranteed to be the user's home. `ensure_config_file()` tries `./server.ini` (CWD) as a fallback, but on launchd this will be `/server.ini` which fails.

## Tasks

### Task 1: macOS CoreAudio — Add play/pause/stop control

**File:** `src/platform/macos.rs`

**Approach:** Use `Arc<AtomicBool>` (same pattern as Linux PipeWire) to gate the render callback.

**Subtasks:**
1. Add `playing: Arc<AtomicBool>` to `CoreAudioAudioOutputStream`
2. Pass `playing` into the render callback closure
3. In the callback: if `!playing`, zero-fill the buffer and return `Ok(())` (same as Linux)
4. Implement `play()` → `playing.store(true, ...)` + ensure stream is started
5. Implement `pause()` → `playing.store(false, ...)`
6. Implement `stop()` → `playing.store(false, ...)` + flush buffer + stop AudioUnit
7. In `start_stream()`, start the AudioUnit **after** creating the stream (not before returning), or use `pause()` to keep it silent until `play()` is called

**Specific code changes:**

Replace the current `CoreAudioAudioOutputStream` struct and impl block with:

```rust
pub struct CoreAudioAudioOutputStream {
    _audio_unit: coreaudio::audio_unit::AudioUnit,
    playing: Arc<AtomicBool>,
}

// In start_stream():
let playing = Arc::new(AtomicBool::new(false));

let ab = audio_buffer.clone();
let vol = volume.clone();
let playing_cb = playing.clone();

audio_unit.set_render_callback(move |data: &mut [f32], _| {
    let is_playing = playing_cb.load(Ordering::SeqCst);
    if !is_playing {
        for sample in data.iter_mut() {
            *sample = 0.0;
        }
        return Ok(());
    }

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
})?;

// Start stream format setup here (before start) ...
audio_unit.start()?;

Ok(Box::new(CoreAudioAudioOutputStream {
    _audio_unit: audio_unit,
    playing,
}))

// Impl block:
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
    // AudioUnit stop is best-effort; the stream object will be dropped
    let _ = self._audio_unit.stop();
}
```

**Checklist:**
- [ ] `cargo check` passes on macOS target
- [ ] `cargo test` passes (macos module tests)
- [ ] Verify `start_stream()` does not produce audio before `play()` is called
- [ ] Verify `pause()` silences output immediately
- [ ] Verify `stop()` terminates the stream cleanly

---

### Task 2: Windows WASAPI — Add play/pause/stop control

**File:** `src/platform/windows.rs`

**Approach:** Same `Arc<AtomicBool>` pattern. The render thread already runs in a loop — just gate it.

**Subtasks:**
1. Add `playing: Arc<AtomicBool>` to `WasapiAudioOutputStream`
2. Pass `playing` into the render thread closure
3. In the render loop: if `!playing`, zero-fill and `WaitForSingleObject` again (sleep until signaled)
4. Implement `play()` → `playing.store(true, ...)`
5. Implement `pause()` → `playing.store(false, ...)`
6. Implement `stop()` → `playing.store(false, ...)` + set `_render_client`/`_audio_client` to stop the stream

**Specific code changes:**

```rust
pub struct WasapiAudioOutputStream {
    _audio_client: wasapi::AudioClient,
    _render_client: wasapi::RenderClient,
    playing: Arc<AtomicBool>,
}

// In start_stream(), add after creating `ab` and `vol`:
let playing = Arc::new(AtomicBool::new(false));
let playing_cb = playing.clone();

// In the render thread loop, at the top:
loop {
    unsafe {
        winapi::um::synchapi::WaitForSingleObject(event, winapi::um::winbase::INFINITE);
    }

    if !playing_cb.load(Ordering::SeqCst) {
        // Buffer is silent — release it and wait again
        let mut buffer = match render_client.get_buffer(buffer_frames as u32) {
            Ok(b) => b,
            Err(_) => break,
        };
        let data = buffer.data_mut();
        for sample in data.iter_mut() {
            *sample = 0.0;
        }
        let _ = render_client.release_buffer(buffer_frames as u32, 0);
        continue;
    }

    // ... existing buffer fill logic ...

// Return:
Ok(Box::new(WasapiAudioOutputStream {
    _audio_client: audio_client,
    _render_client: render_client,
    playing,
}))

// Impl block:
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
}
```

**Checklist:**
- [ ] `cargo check --target x86_64-pc-windows-msvc --no-default-features` passes
- [ ] Verify `play()` starts audio flow
- [ ] Verify `pause()` silences output
- [ ] Verify `stop()` exits the render thread cleanly

---

### Task 3: macOS `init()` — Create runtime directory

**File:** `src/platform/macos.rs`

**Current code (broken):**
```rust
pub fn init() {
    let _dir = runtime_dir();
}
```

**Replace with:**
```rust
pub fn init() {
    ensure_runtime_dir();
}
```

This matches the Linux pattern. `ensure_runtime_dir()` (defined in `src/platform/common.rs:100-115`) creates the directory with `create_dir_all` and sets `0700` permissions on Linux. On macOS the permission-setting block is `#[cfg(target_os = "linux")]`-gated, so it's a no-op on macOS — but the directory IS created, which is what matters.

**Checklist:**
- [ ] `cargo check` passes
- [ ] `runtime_dir()` on macOS returns `~/Library/Caches/l337-audio-server/runtime`
- [ ] Directory is created at startup

---

### Task 4: macOS installer — Write config to the path the binary reads

**File:** `install.sh`, function `setup_launchd()`

**Current behavior:** Installer writes config to `~/Library/Application Support/l337-audio-server/server.ini`, but the binary never reads that path.

**Fix:** Also write the config to `~/.config/l337-audio-server/server.ini` (the `XDG_CONFIG_HOME` location that `load_settings()` checks via `./server.ini` fallback won't work from launchd without `WorkingDirectory`).

Wait — `load_settings()` checks `./server.ini` (CWD), NOT `~/.config/`. And on launchd, CWD is `/`. So `~/.config/` is also not read directly by `load_settings()`.

Let me re-examine `load_settings()`:

```rust
fn load_settings() -> Result<Settings, config::ConfigError> {
    let mut builder = config::Config::builder();
    builder = builder
        .add_source(config::File::with_name("/etc/l337-audio-server/config").required(false))
        .add_source(
            config::File::from(std::path::Path::new("/etc/l337-audio-server/server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        );
    builder = builder
        .add_source(
            config::File::from(std::path::Path::new("server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        )
        .add_source(config::Environment::with_prefix("L337").separator("__"));
```

So the binary reads:
1. `/etc/l337-audio-server/config` (TOML)
2. `/etc/l337-audio-server/server.ini` (INI)
3. `./server.ini` (CWD, INI)
4. `L337__*` env vars

On macOS, there is no `/etc/l337-audio-server/`. The `ensure_config_file()` function creates `/etc/l337-audio-server/server.ini` only if running as root. On macOS launchd, the process runs as the user (not root), so `/etc/...` is not writable. `ensure_config_file()` falls through to `./server.ini` (CWD = `/`), which fails.

So the binary on macOS currently has **no valid config** and falls through to `load_or_create_token()`, which generates a random token and stores it at `~/.cache/l337/l337-audio-server/server_token.txt`.

**The real fix requires one of:**
- Option A: Make the binary also check `~/.config/l337-audio-server/server.ini`
- Option B: Make `ensure_config_file()` create the config in a user-writable location on macOS
- Option C: Have the installer write the config to `/etc/l337-audio-server/server.ini` (requires root, already have it)
- Option D: Use `L337__SERVER__TOKEN` env var in the plist to inject the token directly

**Option D is the simplest and most robust** — it doesn't require changing `load_settings()` or `ensure_config_file()`. The env var wins last in the config chain.

**Implementation — Option D:**

In `setup_launchd()`, after generating the token, write it to the plist:

```xml
<key>EnvironmentVariables</key>
<dict>
    <key>L337__SERVER__TOKEN</key>
    <string>${token}</string>
    <key>HOME</key>
    <string>${real_home}</string>
</dict>
```

This makes the token the installer prints to the user exactly the token the binary uses. No file-path mismatch.

**Also fix `ensure_config_file()` for macOS:**

In `main.rs`, `ensure_config_file()` should also try a user-writable config dir on non-Linux:

```rust
fn ensure_config_file() {
    let etc_path = std::path::Path::new("/etc/l337-audio-server/server.ini");
    if etc_path.exists() {
        return;
    }
    if std::path::Path::new("/etc/l337-audio-server").is_dir() {
        // ... existing write logic ...
        return;
    }

    // User-writable fallback for non-system installs (macOS launchd, etc.)
    if let Some(config_dir) = dirs::config_dir() {
        let config_path = config_dir.join("l337-audio-server").join("server.ini");
        if !config_path.exists() {
            if let Some(parent) = config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&config_path, DEFAULT_CONFIG);
        }
    }

    let cwd_path = std::path::Path::new("server.ini");
    // ... existing CWD fallback ...
}
```

Wait — `ensure_config_file()` is called BEFORE `load_settings()`. If `ensure_config_file()` writes to `~/Library/Application Support/l337-audio-server/server.ini` (on macOS via `dirs::config_dir()`), then `load_settings()` still won't read it because it only checks `/etc/...` and `./server.ini`.

So Option D (env var) is the only fix that doesn't require modifying `load_settings()`. Let me use Option D and also note that Option A (modifying `load_settings()`) is a longer-term improvement.

**Revised plan for Task 4:**

**File:** `install.sh`, function `setup_launchd()`

After generating the token (line ~654), add it to the plist `EnvironmentVariables`:

```xml
<key>EnvironmentVariables</key>
<dict>
    <key>L337__SERVER__TOKEN</key>
    <string>${token}</string>
    <key>HOME</key>
    <string>${real_home}</string>
    <key>XDG_CONFIG_HOME</key>
    <string>${real_home}/.config</string>
    <key>XDG_CACHE_HOME</key>
    <string>${real_home}/.cache</string>
    <key>XDG_STATE_HOME</key>
    <string>${real_home}/.local/state</string>
</dict>
```

This ensures:
- The installer-printed token is the exact token the binary uses
- The binary's `load_or_create_token()` is never reached (token is set via env var)
- `HOME` is set for `dirs::cache_dir()` / `dirs::config_dir()` fallbacks
- XDG vars are set defensively

**Checklist:**
- [ ] `bash -n install.sh` passes
- [ ] `install.sh --dry-run` shows `L337__SERVER__TOKEN` in plist
- [ ] Token printed by installer matches token in plist
- [ ] `load_or_create_token()` is never reached in normal operation (env var wins)

---

### Task 5: macOS installer — Add `WorkingDirectory` to plist

**File:** `install.sh`, function `setup_launchd()`

Add `<key>WorkingDirectory</key>` to the plist so `./server.ini` fallback works if needed:

```xml
<key>WorkingDirectory</key>
<string>${real_home}</string>
```

**Checklist:**
- [ ] Plist validates as XML
- [ ] `launchctl load` succeeds with `WorkingDirectory` set

---

### Task 6: Windows installer — Write config to the path the binary reads

**File:** `install.sh`, function for Windows (currently no Windows-specific function — installer only supports Linux/macOS)

The current installer (`install.sh`) does NOT support Windows. Windows binaries are built by CI but there is no Windows installer script. This is out of scope for this plan — the Windows binary is distributed as a raw artifact.

**However**, the Windows binary still has the same config path problem as macOS:
- `ensure_config_file()` tries `/etc/l337-audio-server/server.ini` (doesn't exist on Windows)
- Falls through to `./server.ini` (CWD depends on how the service is launched)
- `load_or_create_token()` writes to `dirs::cache_dir()` which on Windows is `%LOCALAPPDATA%\l337-audio-server\runtime` or similar

Since there's no Windows installer, the user must manually configure. The env var approach (Option D) would work here too — the user or a future installer can set `L337__SERVER__TOKEN`.

**This task is deferred** — no Windows installer exists, so this is a future improvement. The Windows audio control fix (Task 2) is the priority.

---

### Task 7: Unified test checklist

After all changes, verify on the respective platforms:

**macOS (requires physical Mac or macOS VM):**
- [ ] `cargo build --release --target x86_64-apple-darwin` succeeds with `coreaudio-rs`
- [ ] Binary starts without crashing
- [ ] `play` command starts audio output
- [ ] `pause` command silences audio
- [ ] `stop` command terminates audio
- [ ] Token in `~/Library/Application Support/l337-audio-server/server.ini` matches token in plist
- [ ] `load_or_create_token()` is never reached (check logs)
- [ ] Launchd service survives `launchctl unload/load` cycle
- [ ] Service starts on boot

**Windows (requires physical Windows machine or VM with audio):**
- [ ] `cargo build --release --target x86_64-pc-windows-msvc --no-default-features` succeeds
- [ ] Binary starts without crashing
- [ ] `play` command starts audio output
- [ ] `pause` command silences audio
- [ ] `stop` command terminates audio
- [ ] Token in env/config matches what the client uses

**Linux (CI / existing):**
- [ ] `cargo test` passes (existing tests)
- [ ] No regression in PipeWire behavior

---

## Implementation Order

1. **Task 1** (macOS play/pause/stop) — highest priority, core functionality
2. **Task 3** (macOS init runtime dir) — small fix, do alongside Task 1
3. **Task 4** (macOS token via env var) — fixes config mismatch
4. **Task 5** (macOS WorkingDirectory) — small plist addition
5. **Task 2** (Windows play/pause/stop) — same pattern as Task 1, can be done in parallel
6. **Task 7** (testing) — requires physical hardware

## Files Changed Summary

| File | Change |
|---|---|
| `src/platform/macos.rs` | Add `playing` atomic bool, gate render callback, implement play/pause/stop, call `ensure_runtime_dir()` in `init()` |
| `src/platform/windows.rs` | Add `playing` atomic bool, gate render thread, implement play/pause/stop |
| `install.sh` | Add `L337__SERVER__TOKEN` and `WorkingDirectory` to macOS plist |
| `src/main.rs` | (Optional) Enhance `ensure_config_file()` to also write to user config dir on non-Linux |

## Risks

- **CoreAudio AudioUnit start/stop race:** Starting and stopping the AudioUnit on each play/pause may cause a small audio glitch. Mitigation: keep the AudioUnit running, just gate the callback with the atomic bool (as in the Linux implementation).
- **WASAPI event handle lifetime:** The `event` handle must remain valid for the lifetime of the render thread. `stop()` should not close the handle while the thread is using it. Use `playing` flag + graceful thread exit.
- **Cannot test on macOS/Windows in this environment:** Changes must be validated on physical hardware. `cargo check --target ...` can verify compilation.

## Rollback

All changes are additive — no existing API signatures change. The `AudioOutputStream` trait methods already exist; we're just making them functional instead of no-ops. Revert by restoring the no-op implementations.
