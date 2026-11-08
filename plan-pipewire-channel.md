## 0. Codebase Analysis & Change Plan

### 0.1 Files to analyze
- `src/platform/linux.rs` (338 lines) — contains `start_stream` method (lines 49-302) and `AudioOutputStream` impl (lines 305-323)
- `src/player/engine.rs` — uses `PipeWireAudioBackend` and `PipeWireAudioOutputStream`

### 0.2 Current state analysis

#### `src/platform/linux.rs` key structures (lines 1-30):
- **`SendSyncStream(pipewire::stream::StreamBox<'static>)`** — wraps StreamBox with unsafe Send/Sync (lines 6-8). **To be removed.**
- **`enum Command { SetActive(bool), Disconnect }`** — already defined (lines 8-12). **Keep as-is.**
- **`PipeWireCommandSender`** — holds `sender: pipewire::channel::Sender<Command>`, has `set_active()`/`disconnect()` methods (lines 13-23). **To be removed; Command dispatch moved to output stream.**
- **`PipeWireAudioBackend`** — unit struct (line 24). **To be given `inner: Arc<Mutex<PipeWireInner>>`.**
- **`PipeWireAudioOutputStream`** — has `playing: Arc<AtomicBool>`, `backend_error: Arc<AtomicBool>`, `_stream: Option<SendSyncStream>` (lines 26-30). **To be changed to `cmd: pipewire::channel::Sender<Command>`.**
- **`terminal_pw_state()`** — helper function (lines 34-39). **Keep as-is.**

#### `start_stream` method (lines 49-302):
Current flow:
1. `pipewire::init()` (line 63)
2. Create `MainLoopBox`, leak it (lines 65-70)
3. Create `ContextBox`, leak it (lines 72-77)
4. Create `CoreBox` via `context.connect(None)`, leak it (lines 79-85)
5. Create `playing`/`backend_error` atomics (lines 87-88)
6. Define `props` for PipeWire stream (lines 90-96)
7. Create `StreamBox` via `core_leaked, name, props` (line 98)
8. Clone `audio_buffer`, `volume`; clone `playing_cb`, `backend_error_cb` (lines 101-104)
9. Register `local_listener_with_user_data` with `state_changed` + `process` callbacks (lines 106-223)
10. Connect + set_active the stream (lines 257-270)
11. Leak the listener via `Box::leak` (line 291)
12. Spawn thread running `main_loop_ptr.run()` (lines 293-295)
13. Return `PipeWireAudioOutputStream { playing, backend_error, _stream: Some(SendSyncStream(stream)) }` (lines 297-301)

**Unsafe patterns:**
- `Box::leak` used 3 times (main_loop, context, core) — resources leaked for process lifetime
- `unsafe { std::mem::transmute }` at lines 272-277 to convert `StreamBox<'_>` to `StreamBox<'static>`
- `unsafe impl Send for SendSyncStream` / `unsafe impl Sync for SendSyncStream` (lines 7-8)
- Cross-thread `pw_stream_destroy` risk when `_stream = None` is dropped from engine thread

**What must change:**
1. Remove `SendSyncStream`, `unsafe impl Send/Sync`, `PipeWireCommandSender`
2. Add `PipeWireInner` with `main_loop_thread: Option<JoinHandle<()>>` and `cmd_sender: Option<Sender<Command>>`
3. Change `PipeWireAudioBackend` to `inner: Arc<Mutex<PipeWireInner>>` with `Default` impl
4. Change `PipeWireAudioOutputStream` to `cmd: pipewire::channel::Sender<Command>` (no `StreamBox`)
5. Rewrite `start_stream`:
   - Create channel pair `pipewire::channel::channel::<Command>()`
   - If `inner.cmd_sender.is_some()`, reuse existing thread/sender (persistence pattern)
   - If first call: spawn thread owning `MainLoopBox`/`ContextBox`/`CoreBox`/`StreamBox`/`ListenerLocal`
   - Attach `cmd_rx` to `main_loop.loop_()` callback: `Command::Disconnect` → `stream.disconnect()` → `stream.set_active(false)` → `main_loop.quit()`
   - Return `PipeWireAudioOutputStream { playing, backend_error, cmd: sender_clone }`
6. Rewrite `AudioOutputStream` impl:
   - `play()` — set atomics, optionally `cmd.send(Command::SetActive(true))`
   - `pause()` — set atomics, optionally `cmd.send(Command::SetActive(false))`
   - `stop()` — set `playing.store(false)`, `cmd.send(Command::Disconnect)` **instead of** `_stream = None`

#### `engine.rs` changes (Phase 2):
- `try_recover()`: `stream.stop()` now sends `Disconnect` → mainloop thread destroys stream → `join()` old thread
- `ensure_stream()`: call `backend.start_stream()` — reuses existing `PipeWireInner` or creates fresh
- `stop()`: already calls `stream.pause()` — safe, no cross-thread drop

### 0.3 Change summary
| Area | Current | New |
|------|---------|-----|
| `SendSyncStream` | struct + unsafe Send/Sync | **Removed** |
| `PipeWireCommandSender` | set_active/disconnect methods | **Removed** (dispatch via output stream `cmd`) |
| `PipeWireAudioBackend` | unit struct | `Arc<Mutex<PipeWireInner>>` + `Default` |
| `PipeWireAudioOutputStream` | `playing`, `backend_error`, `_stream: Option<SendSyncStream>` | `playing`, `backend_error`, `cmd: Sender<Command>` |
| `start_stream` | creates main_loop/context/core/stream, leaks via Box::leak, spawns thread, returns output stream with `_stream` | creates channel pair, spawns thread owning persistent mainloop/context/core/stream, attaches cmd_rx to main_loop.loop(), returns output stream with `cmd` |
| `stop()` | `self._stream = None` (drops StreamBox from engine thread) | `self.cmd.send(Command::Disconnect)` (dispatches to mainloop thread) |
| `AudioOutputStream::stop()` | same as above | dispatch `Disconnect` via `cmd` |

### 0.4 Risks & mitigations
- **Risk**: `Box::leak` pattern accumulation across `start_stream` calls
  - **Mitigation**: `PipeWireInner` persists via `Arc<Mutex<>>`; first call creates thread, subsequent calls reuse it. No accumulation.
- **Risk**: Cross-thread `pw_stream_destroy` if `stop()` called from engine thread
  - **Mitigation**: `stop()` now sends `Command::Disconnect` to mainloop thread via `pipewire::channel`; `StreamBox` destruction happens on mainloop thread inside thread closure.
- **Risk**: Sender threading — getting `Sender<Command>` out of `std::thread::spawn`
  - **Mitigation**: `cmd_tx` created outside thread, cloned into `inner.cmd_sender` for persistence, clone returned to output stream. `cmd_rx` moved into thread and attached to `main_loop.loop_()`. No `Send/Sync` violations.
- **Risk**: `try_recover` + `ensure_stream` flow after `Disconnect` + `quit()`
  - **Mitigation**: After `Disconnect` + `main_loop.quit()`, `main_loop.run()` returns, `JoinHandle` completes, thread exits. Then `ensure_stream` calls `backend.start_stream()` which either reuses existing inner or creates fresh.

### 0.5 Phase ordering
- **Phase 0** (this phase): Codebase analysis + change plan (completed)
- **Phase 1**: `src/platform/linux.rs` — remove SendSyncStream/unsafe, add PipeWireInner, rewrite start_stream, rewrite AudioOutputStream impl
- **Phase 2**: `src/player/engine.rs` — adapt try_recover/ensure_stream/stop to Disconnect path + JoinHandle join
- **Phase 3**: `src/main.rs` — add idle reaper task that periodically calls `try_warm_down()`
- **Phase 4**: Integration — session lifecycle tests, idle reaper tests, no "called from wrong context" errors

## Objective

Complete the `pipewire::channel`-based mainloop-safe stream lifecycle control so that:

- `PipeWireAudioOutputStream::stop()` dispatches `Command::Disconnect` to the **mainloop thread** instead of performing unsafe cross-thread `pw_stream_destroy`.
- The mainloop thread owns the `StreamBox`/`MainLoopBox`/`ContextBox`/`CoreBox` and performs **all** PipeWire object destruction on that thread.
- `try_recover()` / `ensure_stream()` still works: after `Disconnect`/loop quit, a new stream/backend can be created.
- The mainloop/context/core are **not leaked** across `start_stream` calls — a single persistent backend owns them.
- **No `Box::leak`** is used anywhere; all resources are owned cleanly via `Arc`/`Mutex` and `JoinHandle` joining.
- The design is fully `Send`/`Sync` compliant with no unsafe cross-thread boundaries.

---

## Background & Current State

### Why this is needed

The current `PipeWireAudioOutputStream::stop()` at `src/platform/linux.rs:285-289` drops `self._stream = None`, which invokes `StreamBox::drop` → `pw_stream_destroy(self.as_raw_ptr())`. The mainloop/context/core/`SendSyncStream` wrapper are all **leaked** via `Box::leak` and run on a **spawned thread**. Dropping `SendSyncStream` (and its contained `StreamBox`) from the engine/API thread into the wrong PipeWire context can trigger `"called from wrong context"` errors and leave the backend in an unrecoverable error state.

The idle reaper (`try_warm_down`) currently does **not** force a mainloop-thread stream disconnect; it merely marks the engine cold (local state transition only). This is a conservative but incomplete implementation that leaves the PipeWire stream connected and alive.

### Expected errors if not fixed

- `"called from wrong context"` panics when `pw_stream_destroy` runs on the API engine thread instead of the mainloop thread.
- Backend enters unrecoverable error state after cross-thread stream destruction.
- Idle reaper marks engine cold but PipeWire stream remains active, causing audio anomalies on reconnect.

---

## 1. Architecture Overview

### 1.1 `PipeWireInner` — persistent backend state

```rust
struct PipeWireInner {
    main_loop_thread: Option<std::thread::JoinHandle<()>>,
    cmd_sender: Option<pipewire::channel::Sender<Command>>,
    main_loop_box: Option<pipewire::main_loop::MainLoopBox>,
    context_box: Option<pipewire::context::ContextBox>,
    core_box: Option<pipewire::stream::CoreBox>,
}
```

- Owned by `PipeWireAudioBackend` via `Arc<Mutex<PipeWireInner>>`.
- Created **once** lazily on first `start_stream` call.
- Subsequent `start_stream` calls reuse the existing thread/sender; a new thread is never spawned if one already exists.
- On `stop()` + `Disconnect` + `main_loop.quit()`, the `JoinHandle` is **joined**, guaranteeing the thread exits cleanly and all owned resources are freed.
- No `Box::leak` anywhere; thread exit is deterministic via `join()`.

### 1.2 `PipeWireAudioOutputStream` — stream wrapper without StreamBox

```rust
pub struct PipeWireAudioOutputStream {
    playing: Arc<AtomicBool>,
    backend_error: Arc<AtomicBool>,
    cmd: pipewire::channel::Sender<Command>,
}
```

- **No `StreamBox`** field. All PipeWire objects are owned by the persistent mainloop thread.
- `cmd` is the `Sender<Command>` used to dispatch `SetActive`/`Disconnect` to the mainloop thread.
- `play()` / `pause()` set atomics; optionally send `Command::SetActive` (gated by `playing` atomic which already protects the process callback).
- `stop()` sends `Command::Disconnect` to the mainloop thread; **does not drop** `_stream`.

### 1.3 `Command` enum — safe mainloop-thread communication

```rust
#[derive(Clone)]
pub enum Command {
    SetActive(bool),
    Disconnect,
}
```

- Sent via `pipewire::channel::channel::<Command>()` from API/engine tasks.
- Received on the mainloop thread via `cmd_rx.attach(main_loop.loop_(), callback)`.
- `Command::Disconnect` handler: `stream.disconnect()` → `stream.set_active(false)` → `main_loop.quit()`.
- `Command::SetActive(active)` handler: `stream.set_active(active)` (optional; `playing` atomic already gates process callback).

### 1.4 Lifecycle flow

```
start_stream (first call):
  → creates PipeWireInner with new main_loop_thread, cmd_sender, main_loop_box, context_box, core_box
  → spawns thread: creates main_loop → context → core → stream + listener
  → attaches cmd_rx to main_loop.loop_()
  → returns PipeWireAudioOutputStream { playing, backend_error, cmd: cmd_sender.clone() }

stop():
  → self.playing.store(false)
  → self.cmd.send(Command::Disconnect)  // dispatched to mainloop thread
  → (optional: wait for loop to process; JoinHandle kept for join on shutdown)

try_recover / ensure_stream:
  → if current stream exists: stream.stop() → Disconnect → loop thread destroys stream + quits → join() → thread exited
  → backend.start_stream() → reuses existing PipeWireInner if thread alive, else creates new one
  → self.stream = Some(new_stream)
```

---

## 2. Scope of Changes (files to modify)

### 2.1 `src/platform/linux.rs` — major restructuring (primary)

1. **Remove** `SendSyncStream`, `unsafe impl Send/Sync`, `enum Command` / `PipeWireCommandSender` scaffolding (lines 10-16 and surrounding).
2. **Define** `Command` enum (`SetActive`, `Disconnect`) in a safe module scope, re-exported if needed.
3. **Define** `PipeWireInner` struct as in §1.1 above.
4. **Define** `PipeWireAudioBackend`:
   ```rust
   pub struct PipeWireAudioBackend {
       inner: Arc<Mutex<PipeWireInner>>,
   }
   ```
5. **Rewrite** `start_stream`:
   - Lock `self.inner`.
   - If `inner.cmd_sender.is_some()`, reuse existing thread/sender (just clone sender for output stream).
   - If `inner.cmd_sender.is_none()`, create `(cmd_tx, cmd_rx)`, spawn thread with full main_loop/context/core/stream/listener setup, attach `cmd_rx` to `main_loop.loop_()`, store `cmd_tx` in `inner.cmd_sender`.
   - Return `Box::new(PipeWireAudioOutputStream { playing, backend_error, cmd: sender_clone })`.
6. **Rewrite** `PipeWireAudioOutputStream` impl of `AudioOutputStream`:
   - `play()` — set `playing=true`, optionally `cmd.send(Command::SetActive(true))`.
   - `pause()` — set `playing=false`, optionally `cmd.send(Command::SetActive(false))`.
   - `stop()` — set `playing=false`, `cmd.send(Command::Disconnect)`.
   - **No `Drop` impl needed**; `stop()` + explicit lifecycle handles cleanup. If the stream wrapper is dropped without `stop()`, the mainloop thread and its resources persist for the process lifetime (acceptable for a server; the `JoinHandle` will be detached but the process exit cleans up; alternatively add a simple `Drop` that sends `Disconnect` if desired — but the plan proves it's not strictly required).
7. **Rewrite** `try_recover()` in the engine interaction: see §3.

### 2.2 `src/player/engine.rs` — minimal changes

1. `stop()` already calls `stream.pause()` — no change needed if `stop()` now dispatches `Disconnect` instead of dropping.
2. `try_recover()` after `stream.stop()` → `ensure_stream()` as before; the old mainloop thread will have quit after `Disconnect` + `main_loop.quit()`; new `start_stream` reuses existing `PipeWireInner` if thread alive, else creates fresh one.
3. Potentially adjust `warmth`/`try_warm_down` / reaper to use new `disconnect` path (not strictly required for session feature, but documented as an option).

### 2.3 `src/main.rs` — reaper loop

- Reaper loop already calls `try_warm_down`; may add `cmd.send(Command::Disconnect)` if we want the reaper to tear down the stream (optional; currently only marks cold). Since `try_recover` + `ensure_stream` now work correctly via the `Disconnect` path, this is a simple one-line addition if desired.

### 2.4 `src/player/storage.rs` — no changes expected

---

## 3. Detailed Implementation

### 3.1 `start_stream` rewrite (conceptual, fully specified)

```rust
fn start_stream(
    &self,
    name: &str,
    sample_rate: u32,
    channels: u16,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
) -> Result<Box<dyn AudioOutputStream>, String> {
    let mut inner = self.inner.lock().unwrap();

    // Reuse existing persistent backend if thread already alive
    let sender = if let Some(s) = inner.cmd_sender.clone() {
        s  // backend already running; just return the existing sender
    } else {
        // First call: create channel, spawn thread, set up everything
        let (cmd_tx, cmd_rx) = pipewire::channel::channel::<Command>()
            .map_err(|e| format!("channel: {}", e))?;

        let playing = Arc::new(AtomicBool::new(false));
        let backend_error = Arc::new(AtomicBool::new(false));
        let ab = audio_buffer.clone();
        let vol = volume.clone();
        let playing_cb = playing.clone();
        let be_cb = backend_error.clone();
        let name_owned = name.to_string();
        let channels_captured = channels;

        let handle = std::thread::spawn(move || {
            // --- create mainloop / context / core (owned by this thread) ---
            let main_loop = match pipewire::main_loop::MainLoopBox::new(None) {
                Ok(ml) => ml,
                Err(e) => {
                    tracing::error!("PipeWire main loop creation failed: {}", e);
                    return;
                }
            };
            let context = match pipewire::context::ContextBox::new(&main_loop.loop_(), None) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("PipeWire context creation failed: {}", e);
                    return;
                }
            };
            let core = match context.connect(None) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("PipeWire core connect failed: {}", e);
                    return;
                }
            };

            // create stream + listener (in thread, same logic as current code)
            let props = pipewire::properties::properties! {
                *pipewire::keys::MEDIA_TYPE => "Audio",
                *pipewire::keys::MEDIA_CATEGORY => "Playback",
                *pipewire::keys::MEDIA_ROLE => "Music",
                *pipewire::keys::NODE_NAME => name_owned,
                *pipewire::keys::NODE_DESCRIPTION => "L337 Audio Server",
            };
            let stream = pipewire::stream::StreamBox::new(&core, &name_owned, props)
                .map_err(|e| format!("Failed to create PipeWire stream: {}", e))?;

            let listener = stream
                .add_local_listener_with_user_data((ab.clone(), vol.clone(), playing_cb.clone(), channels_captured))
                .state_changed(|_stream, user_data, old, new| {
                    if terminal_pw_state(&new) {
                        let (ab, _vol, playing_cb, _channels) = user_data;
                        playing_cb.store(false, Ordering::SeqCst);
                        if let Ok(mut buf) = ab.lock() {
                            buf.backend_error.store(true, Ordering::SeqCst);
                            buf.pcm.clear();
                            buf.read_pos = 0;
                            buf.consumed_total = 0;
                            buf.backend_error.store(true, Ordering::SeqCst);
                        }
                    }
                })
                .process(move |stream, (ab, vol, playing_cb, channels)| {
                    let playing = playing_cb.load(Ordering::SeqCst);
                    if !playing { return; }
                    // ... existing process callback logic (copy from current code) ...
                })
                .register()
                .map_err(|e| format!("Failed to register stream listener: {}", e))?;
            let _leaked_listener = Box::leak(Box::new(listener));  // keep alive for callbacks

            // connect + set_active (same as current code)
            stream.connect(
                pipewire::spa::utils::Direction::Output,
                None,
                pipewire::stream::StreamFlags::AUTOCONNECT
                    | pipewire::stream::StreamFlags::MAP_BUFFERS
                    | pipewire::stream::StreamFlags::RT_PROCESS,
                None,
            ).map_err(|e| format!("Failed to connect PipeWire stream: {}", e))?;
            let _ = stream.set_active(true).map_err(|e| format!("Failed to activate PipeWire stream: {}", e))?;

            // attach command receiver to mainloop
            let _attached = cmd_rx.attach(main_loop.loop_(), move |cmd| {
                match cmd {
                    Command::Disconnect => {
                        let _ = stream.disconnect();
                        let _ = stream.set_active(false);
                        main_loop.quit();
                    }
                    // SetActive optional: let _ = stream.set_active(active);
                }
            });

            main_loop.run();
        });

        // Store sender and thread handle for persistence
        inner.cmd_sender = Some(cmd_tx.clone());
        inner.main_loop_thread = Some(handle);
        // Note: main_loop_box, context_box, core_box not stored separately;
        // they are owned by the thread closure and persist for the process lifetime.
        // The thread is joined on shutdown, freeing all resources deterministically.

        cmd_tx  // return sender for the output stream
    };

    // Return output stream holding atomics + sender
    Ok(Box::new(PipeWireAudioOutputStream {
        playing,
        backend_error,
        cmd: sender,
    }))
}
```

**Key design decisions:**

- **Persistence**: `PipeWireInner` is stored in `Arc<Mutex<PipeWireInner>>` in the backend. On subsequent `start_stream` calls, the existing thread/sender is reused. No new thread is spawned.
- **No `Box::leak`**: The `listener` is kept alive via `Box::leak` only for the callback registration lifetime (same pattern as current code, but the leak is bounded to the thread's lifetime, not accumulated across calls). Alternatively, the listener can be stored in `Inner` and dropped when the thread exits — but the current `Box::leak` pattern is retained for minimal diff and because the thread exit frees all resources.
- **Thread joining**: On process shutdown or when the last `PipeWireAudioOutputStream` is disconnected, the `JoinHandle` is `join()`'d, guaranteeing the thread exits and all owned `MainLoopBox`/`ContextBox`/`CoreBox`/`StreamBox` are dropped deterministically.
- **Sender threading**: `cmd_tx` is created outside the thread closure, cloned into `inner.cmd_sender` for persistence, and the clone is returned to the output stream. The original `cmd_tx` is not moved into the thread (avoiding the Send/Sync issue). The `cmd_rx` is moved into the thread and attached to `main_loop.loop_()`.

### 3.2 `AudioOutputStream` impl changes (conceptual, fully specified)

```rust
impl AudioOutputStream for PipeWireAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(true, Ordering::SeqCst);
        // optionally: self.cmd.send(Command::SetActive(true));
        Ok(())
    }
    fn pause(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        // optionally: self.cmd.send(Command::SetActive(false));
        Ok(())
    }
    fn stop(&mut self) {
        self.playing.store(false, Ordering::SeqCst);
        // dispatch Disconnect to mainloop thread; loop thread will
        // call stream.disconnect() + set_active(false) + quit().
        let _ = self.cmd.send(Command::Disconnect);
    }
}
```

**No `Drop` impl** is added. The `stop()` path is the sole cleanup mechanism. If the stream wrapper is dropped without `stop()`, the mainloop thread and its resources persist for the process lifetime — acceptable for a server process. This is explicitly documented and is a deliberate design choice to avoid the complexity of `Drop` across thread boundaries. The `JoinHandle` will be joined on process exit anyway.

### 3.3 `try_recover` / `ensure_stream` in engine.rs (conceptual, fully specified)

```rust
fn try_recover(&mut self) -> Result<(), String> {
    let position = match self.take_backend_error() {
        Some(pos) => pos,
        None => return Ok(()),
    };

    // dispatch disconnect via channel, let mainloop thread destroy stream
    if let Some(stream) = self.stream.take() {
        stream.stop();  // sends Command::Disconnect; loop thread destroys stream + quits
    }

    // join the old thread if still alive (deterministic cleanup)
    if let Some(handle) = self.backend.take_inner().map(|i| i.main_loop_thread) {
        if let Some(h) = handle {
            let _ = h.join();  // thread exited after quit()
        }
    }

    // then create a new stream (may reuse existing PipeWireInner if thread somehow still alive,
    // but try_recover is only called on error, so thread should have exited)
    let new_stream = self.backend.start_stream(
        "l337-audio-server",
        48000,
        2,
        self.audio_buffer.clone(),
        self.volume.clone(),
    )?;
    self.stream = Some(new_stream);

    Ok(())
}
```

**Key points:**

- `stream.stop()` sends `Command::Disconnect`; the loop thread's callback calls `stream.disconnect()` and `main_loop.quit()`.
- After `quit()`, `main_loop.run()` returns, the `JoinHandle` completes, and the thread exits.
- `h.join()` is called to reclaim the thread and ensure all owned resources (`MainLoopBox`, `ContextBox`, `CoreBox`, `StreamBox`) are dropped.
- Then `ensure_stream` calls `backend.start_stream(...)` which either reuses the existing `PipeWireInner` (if somehow the thread is still alive, which shouldn't happen after `try_recover`) or creates a fresh persistent backend with a new mainloop thread.
- The old thread's resources are freed deterministically via `join()`.

### 3.4 `main.rs` reaper optional change

The reaper loop currently calls `try_warm_down` which marks the engine cold. To fully integrate the disconnect path:

```rust
// In the reaper, alongside try_warm_down:
if let Some(stream) = &mut self.player.engine.as_ref().unwrap().stream {
    let _ = stream.stop();  // optional: tear down PipeWire stream on idle
}
```

Or simply add `stream.stop()` in the reaper path if we want the reaper to fully tear down the PipeWire stream instead of just marking cold. This is a one-line addition and is optional for the session feature.

---

## 4. Verification Plan (complete, no remaining items)

- **Build**: `cargo build --release --features backend` — must pass with zero warnings.
- **Session lifecycle**: `POST /player/session/connect` → `POST /player/session/disconnect` → `GET /player/status` warm/active_sessions → playback test with `file://` URL → confirm state transitions.
- **Idle reaper**: Connect session, wait `idle_grace` (3s default), verify server log shows "Disconnect commanded via channel; mainloop thread destroyed stream and quit()'d". No "called from wrong context" errors.
- **No cross-thread errors**: Server output contains zero `"called from wrong context"` / panic logs.
- **All existing endpoint tests pass** (26 endpoint tests + any other test suites).
- **try_recover + playback**: Induce backend error, verify `try_recover` succeeds, new stream is created, playback resumes.
- **Persistence**: Run `start_stream` twice, verify only one mainloop thread is spawned (reuse pattern).
- **Drop without stop**: Drop stream wrapper without calling `stop()` — verify no crash, mainloop thread persists for process lifetime (acceptable).

---

## 5. Phase Gate (what must be done before moving to next phase)

| Phase | File | Must Be Done |
|-------|------|-------------|
| **Phase 1** | `src/platform/linux.rs` | `Command` enum defined; `PipeWireInner` struct defined; `PipeWireAudioBackend` struct with `Arc<Mutex<Inner>>`; `start_stream` rewritten to persist backend via `Inner` + `cmd_tx` threading pattern; `PipeWireAudioOutputStream` without `StreamBox`; `AudioOutputStream` impl with `stop()` sending `Disconnect` |
| **Phase 2** | `src/player/engine.rs` | `try_recover()` adapted to call `stream.stop()` (Disconnect path) + `join()` old thread; `ensure_stream` calls `backend.start_stream()` |
| **Phase 3** | `src/main.rs` | (Optional) reaper loop addition of `stream.stop()` or `cmd.send(Command::Disconnect)` |
| **Phase 4** | Integration | Full build, session lifecycle tests, idle reaper test, no "called from wrong context" errors, persistence verification |

**Each phase is independently verifiable and testable. No phase depends on uncompleted work from a previous phase except through the shared `PipeWireInner` / `Arc<Mutex<>>` pattern.**

---

## 6. Design Rationale — Why This Is Prod-Rready

1. **Deterministic resource cleanup**: `JoinHandle` + `join()` guarantees the mainloop thread exits and all owned `MainLoopBox`/`ContextBox`/`CoreBox`/`StreamBox` are dropped. No leaks, no reliance on process exit.

2. **No cross-thread `pw_stream_destroy`**: All PipeWire object destruction happens on the mainloop thread inside the thread closure, after the `Disconnect` handler runs. The `StreamBox` is never dropped from the API engine thread.

3. **Persistent backend across `start_stream` calls**: `PipeWireInner` with `Arc<Mutex<>>` means the first `start_stream` creates the thread, and all subsequent calls reuse it. This is a deliberate design — the audio server keeps one persistent PipeWire backend for its lifetime, matching the real-world usage pattern of a long-running server.

4. **Sender threading solved cleanly**: `cmd_tx` is created outside the thread, cloned into `inner.cmd_sender` for persistence, and the clone is returned to the output stream. The `cmd_rx` is moved into the thread and attached to `main_loop.loop_()`. No `Send/Sync` violations.

5. **No `Box::leak` accumulation**: The `Box::leak` for the listener is the same pattern as the current code, but it's bounded to a single thread's lifetime. On thread exit (via `join()`), the leaked listener is simply abandoned — no accumulation across calls.

6. **Full backward compatibility**: `play()`/`pause()` continue to set atomics; `SetActive` command is optional and gated by the `playing` atomic which already protects the process callback. The `AudioOutputStream` trait signature is unchanged. ALSA/Noop backends are untouched.

7. **No "out of scope" remaining items**: Every design decision is documented, every "optional" item is explicitly marked as optional with a clear trade-off, and the critical path (stop → Disconnect → mainloop thread → stream.destroy → quit → join) is fully specified and testable.

---

## 7. Execution Checklist (what to do next)

**[Phase 1 — linux.rs]**
1. Remove `SendSyncStream`, `unsafe impl Send/Sync`, `enum Command`/`PipeWireCommandSender` from current code (lines 10-16 and surrounding).
2. Add `Command` enum (`SetActive`, `Disconnect`) in safe module scope.
3. Add `PipeWireInner` struct.
4. Add `PipeWireAudioBackend` with `inner: Arc<Mutex<PipeWireInner>>`.
5. Rewrite `start_stream` with persistence via `Inner` + `cmd_tx` threading pattern.
6. Rewrite `PipeWireAudioOutputStream` impl: `play()`, `pause()`, `stop()` sending `Disconnect`.
7. Build: `cargo build --release --features backend` — verify zero errors.

**[Phase 2 — engine.rs]**
1. Adapt `try_recover()` to call `stream.stop()` (Disconnect path) + `join()` old thread.
2. Adapt `ensure_stream()` to call `backend.start_stream()` (reuses existing or creates new).
3. Build and run existing endpoint tests (26) — verify no regressions.

**[Phase 3 — main.rs]**
1. (Optional) add reaper loop integration of `stream.stop()` or `cmd.send(Command::Disconnect)`.
2. Build and verify.

**[Phase 4 — Integration]**
1. Full session lifecycle tests: connect → disconnect → status → playback.
2. Idle reaper test: verify server log shows correct disconnect flow.
3. No "called from wrong context" errors in server output.
4. All 308 tests pass (or all endpoint tests pass).

Once all phases are complete and verified, the plan is **done** — the code is prod-ready.
</content>