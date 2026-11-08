# Plan: Cool Up / Cool Down (Session Warmth Lifecycle)

**Status:** partially implemented (production session/warmth API + idle reaper state transition done; safe PipeWire mainloop disconnect/channel integration and deployment verification remain)

**Scope:** `l337-audio-server` (authoritative warmth/session lifecycle, PipeWire backend control) and `l337-player` (session connect/disconnect client). Feature spans both repos; the remaining backend work is the critical path.

**Related files (player):**
- `coolup-cooldown-sessoin.md` (session transcript / context; not a formal spec)
- `src/client/core/api_client.py` — `session_connect()` / `session_disconnect()`
- `src/client/tui/audio_server_manager.py` — best-effort connect/disconnect on verified auth/reachability and teardown

**Related commits (audio server, existing):**
- `963c711` — warmth framework introduced (`ColdWarm`, fields, helpers); helpers were misplaced in `#[cfg(test)]` and semantics needed correction.
- `4194a87` — idle reaper placeholder in `src/main.rs`.
- `6f44e97` — unintegrated `PipeWireCommandSender`/`Command` scaffolding in `src/platform/linux.rs`.

---

## Objective

Complete production-grade cool up/cool down behavior:

1. A player client can announce session lifecycle to the audio server so the server stays warm while active clients exist and can cool down when idle.
2. The engine tracks active sessions and warmth state safely.
3. Authenticated player activity (`status`, playback entry points, session calls) keeps the server warm without corrupting session counts.
4. An idle reaper cools the engine down after `idle_grace` when stopped and no active sessions remain.
5. PipeWire stream control/teardown is performed only in backend-safe contexts (mainloop thread/channel), never via unsafe cross-thread `pipewire::stream`/mainloop object destruction from arbitrary tasks.
6. Player and server contracts match; behavior is covered by automated tests and verified on a real PipeWire/socket deployment.

---

## Implemented (current working tree, uncommitted in `~/git/l337-audio-server`)

> **Repo state note:** These changes are currently modified in the working tree on `main` (`src/api/handlers.rs`, `src/api/models.rs`, `src/main.rs`, `src/player/engine.rs`, `tests/endpoints.rs`). They are **not committed**. Committing them (with an appropriate message/scope split if desired) is part of remaining work.

### Engine (`src/player/engine.rs`)

- Warmth helpers moved/corrected into **production** `impl PlayerEngine` (removed misplaced test-module `impl`):
  - `touch()` — records `last_client_contact = Instant::now()` and sets `warmth = Warm`. **Does not** increment `active_sessions` (fixes prior incorrect behavior).
  - `warm_up() -> Result<(), String>` — `active_sessions = saturating_add(1)`, `touch()`, `ensure_warm()`.
  - `cool_down()` — `active_sessions = saturating_sub(1)`, `touch()`.
  - `active_sessions() -> u32`, `is_warm() -> bool`.
  - `ensure_warm() -> Result<(), String>` — touches, reuses existing stream or creates one via `backend.start_stream(...)`, clears stale `backend_error`, updates `warmth/state` on failure. **Only creates/reuses handles; does not perform cross-thread teardown.**
  - `try_warm_down(now) -> bool` — returns `false` unless `active_sessions == 0`, `state == Stopped`, and idle past `idle_grace`. On success, clears PCM/read state and sets `warmth = Cold`. **Does not drop/disconnect the PipeWire stream handle** (deferred to backend-safe control paths) to avoid unsafe cross-thread teardown.
- `play_track()` and `get_status()` call `touch()` to keep active/polling sessions warm.

### API / routing (`src/api/handlers.rs`, `src/main.rs`)

- `POST /player/session/connect` → `engine.warm_up()`; returns JSON `{ "ok": true, "warm": <bool>, "active_sessions": <u32> }` on success, `500 {ok:false,error}` on warm-up failure.
- `POST /player/session/disconnect` → `engine.cool_down()`; returns `{ok,warm,active_sessions}`.
- Routes registered in `main.rs`.

### Status model (`src/api/models.rs`, `src/player/engine.rs`)

- `PlayerStatus` now includes:
  - `warm: bool` (`#[serde(default)]`)
  - `active_sessions: u32` (`#[serde(default)]`)
- `get_status()` populates these from engine state.

### Idle reaper (`src/main.rs`)

- Recovery loop now checks stopped state and calls `engine.try_warm_down(Instant::now())`; logs when the engine becomes cold after inactivity.
- Reaper currently performs **engine-local** cool-down only (safe). It does **not** yet send a backend/mainloop disconnect command.

### Tests

- Added `tests/endpoints.rs::test_session_connect_disconnect_and_status`:
  - connect → `ok`, `active_sessions >= 1`
  - status exposes `warm` and `active_sessions`
  - disconnect → `ok`, `active_sessions == 0`
- Verification performed:
  - `cargo test --test endpoints` → **26 passed**
  - `cargo fmt` applied
  - Player `./run-test.sh tests/unit/test_audio_server_autostart.py -q` → **17 passed** (no player code changes needed; client already calls session endpoints)

---

## Client contract (player) — already compatible

- `APIClient.session_connect()` → `POST /player/session/connect`
- `APIClient.session_disconnect()` → `POST /player/session/disconnect`
- `audio_server_manager.py` announces session only after reachability/auth verification and performs best-effort disconnect on teardown.

**Compatibility note:** Server now returns `warm`/`active_sessions` in status and session responses; client should ignore unknown fields (typical for JSON clients). Confirm player parsing remains tolerant if any typed status model is strict.

---

## Remaining Work

### 1) PipeWire mainloop-safe control channel (critical, highest risk)

**Problem:** PipeWire `stream`/mainloop/context/core objects and activation/disconnect operations are not safe to mutate/drop from arbitrary Tokio/API threads. Current Linux backend (`src/platform/linux.rs`) leaks mainloop/context/core, spawns a thread running the loop, and `PipeWireAudioOutputStream::stop()` sets `playing=false` and drops `_stream` from the caller thread. Unintegrated scaffolding exists (`Command::{SetActive,Disconnect}`, `PipeWireCommandSender`) but is malformed/unused.

**Goal:** All PipeWire stream lifecycle/control that must run on the mainloop thread is dispatched via `pipewire::channel` (or equivalent PipeWire mainloop integration). API/engine/reaper code must never directly tear down PipeWire objects across threads.

**Concrete tasks:**

1. **Clean up scaffolding** in `src/platform/linux.rs:10` (currently one-line malformed `enum`/`struct`/`impl`).
2. **Create channel pair** during `PipeWireAudioBackend::start_stream`:
   - `pipewire::channel::channel::<Command>()` (or `sync_channel` if appropriate for the crate API/version).
   - Attach receiver to the PipeWire mainloop so commands are handled on the loop thread.
3. **Define command semantics** (minimum viable):
   - `SetActive(bool)` — call `stream.set_active(active)` on loop thread.
   - `Disconnect` / `Teardown` — pause/deactivate, disconnect stream, and perform loop-thread-safe cleanup as supported by the `pipewire` crate/version in use. Avoid leaking resources if safe cleanup is possible; if not possible with current structure, document exact limitation and ensure no UB/cross-thread teardown.
   - Optional: `Play`/`Pause` if needed, but current `playing` atomic already gates process callback; prefer minimal change.
4. **Expose sender to control path**:
   - Store sender in `PipeWireAudioOutputStream` (and/or return it through a backend abstraction) so `play/pause/stop` and warm-down/disconnect can dispatch commands.
   - This likely requires extending `AudioOutputStream`/backend traits in `src/platform/common.rs` and updating all platform implementations (`linux.rs`, `dummy`/other platform backends if present) to compile.
5. **Implement thread-safe `AudioOutputStream` behavior**:
   - `play()`/`pause()` should set atomics and, where required, dispatch `SetActive`.
   - `stop()`/new `disconnect()` must **not** drop PipeWire stream objects off-thread; it should dispatch a loop-thread command and mark local state inactive. Dropping Rust wrappers may still be unsafe depending on PipeWire bindings — verify.
6. **Integrate engine/reaper warm-down**:
   - When `try_warm_down`/idle reaper determines the engine should cool, invoke a backend-safe disconnect path (new engine method or stream control hook). Keep state transitions serialized under engine lock; keep PipeWire calls off the engine/API thread.
   - Consider adding a queued “disconnect requested” flag/command handle accessible to `main.rs` without blocking playback paths.
7. **Handle recovery/recreate**:
   - After disconnect, `ensure_warm()`/`ensure_stream()`/`try_recover()` must be able to create a fresh stream cleanly. Verify no stale leaked listener/loop state breaks reconnection.

**Risks / constraints:**
- `pipewire` Rust crate API details (`channel` attachment, stream/mainloop ownership, `'static`/leak patterns) must match the pinned dependency version in `Cargo.toml`/lockfile.
- Leaked `main_loop/context/core/listener` are currently used to satisfy lifetimes; refactoring may require `Rc`/loop-owned structures on the mainloop thread instead of `Box::leak`.
- `unsafe impl Send/Sync` for `SendSyncStream` is only sound if access is properly synchronized and mainloop-thread constraints are respected; refactor should reduce/justify unsafe.
- Real validation requires PipeWire hardware/session (`--features backend`/`pipewire`) and observing no `called from wrong context` / deadlock / use-after-free symptoms.

**Acceptance criteria:**
- No cross-thread PipeWire stream/mainloop mutation from API tasks, engine methods running off the mainloop thread, or reaper.
- Idle reaper can move a stopped, sessionless, idle engine to cold **and** disconnect the backend stream safely.
- Playback start/resume/recovery still works after cool-down.
- `cargo test` and endpoint tests pass; manual PipeWire/socket smoke test passes.

### 2) Engine lifecycle semantics / hardening

1. **Session count correctness under client crashes**: `session_disconnect` is best-effort; crashed clients may leak `active_sessions`. Options:
   - session IDs/leases with TTL/heartbeat (larger change),
   - or bound behavior and expose metrics; at minimum document. Reaper should not cool down while `active_sessions > 0`, so leaked sessions can prevent cool-down — acceptable fail-safe but operationally visible.
2. **`touch()` on every `get_status()`** keeps server warm while polling; confirm intended (matches current design) and ensure idle reaper grace is sensible (default 90s).
3. **Warm-up on playback entry points**: `play_track` touches but does not require a registered session; `warm_up` (session connect) creates stream. Validate desired behavior for clients that play without explicit session connect (if any).
4. **Thread-safety of `PlayerEngine` methods now called from more paths**: engine remains behind `tokio::sync::Mutex` in `AppState` — good; avoid holding lock across long awaits except where already necessary.

### 3) Player-side validation / follow-ups (`/mnt/private/data/home/tim/git/l337-player`)

1. Confirm `PlayerStatus`/status parsing tolerates new `warm`/`active_sessions` fields (serde/defaults on server are fine; check any strict typed models in player).
2. Consider surfacing warmth/session diagnostics only in debug logs/UI diagnostics, not user-facing noise.
3. Run relevant player tests beyond autostart if available (search/nonblocking/playback regressions) via `./run-test.sh`.
4. Ensure branch/release flow is clear: player `1-task-session-api` is ahead 1 (spec/context file commit) and already has client session calls; audio server changes are uncommitted on `main`. Coordinate merge order (server should deploy compatible endpoints before/with client reliance).

### 4) CI / build / feature alignment (`l337-audio-server`)

1. **Linux PipeWire production builds** must include the real backend (`--features backend` and/or `pipewire` as appropriate). Current workflow excerpt shows PipeWire/ALSA variants with `backend,opus` / `alsa,opus` — verify every shipped Linux artifact that targets PipeWire actually enables PipeWire backend code paths.
2. **Cargo features**: `default = []`, `backend = ["pipewire","coreaudio-rs","wasapi"]`, `alsa = []`. Confirm `dummy`/no-hardware behavior and `PlayerEngine::new`/backend selection are correct for each target.
3. Add/adjust CI test coverage if session endpoint tests should run in CI (they are normal `cargo test --test endpoints` tests; verify CI invokes them).

### 5) Deployment / ops (requires sudo / environment access)

1. Rebuild backend-enabled fixed binary and install to `/opt/l337-audio-server/l337-audio-server` (or distro/service path).
2. Restart system service; confirm it serves `unix:///run/user/1002/l337.sock` (or configured socket) with correct token/capabilities.
3. Smoke test:
   - `GET /health` (unauthenticated, as designed)
   - authenticated `POST /player/session/connect` / `GET /player/status` / `POST /player/session/disconnect`
   - play/stop/idle, observe `warm`/`active_sessions` and post-reaper behavior
   - on PipeWire host, confirm no `called from wrong context` errors and audio device/stream behavior is stable.
4. **Installed `/opt/...` binary was previously old/noop** — treat as stale until replaced.

### 6) Housekeeping

1. Commit audio server changes with clear message(s), e.g. session/warmth API + reaper state machine (and separately channel refactor when done).
2. Remove or properly integrate `PipeWireCommandSender`/`Command` scaffolding formatting.
3. Evaluate `cargo fix`able warnings (14 warnings reported on `cargo check`) — only if safe/unrelated changes are acceptable.
4. Create/update authoritative gap tracking if the team expects `gaps.md` (prior summary referenced it; file not found).

---

## Verification checklist

- [x] `cargo test --test endpoints` (audio server) — includes session lifecycle test.
- [x] Player `test_audio_server_autostart.py` passes.
- [ ] Full `cargo test` in audio server green in CI-equivalent environment (note previously flaky/unrelated `test_play_track_short_circuits_on_youtube_block` in full run history; investigate only if reproducible/regressed).
- [ ] Manual PipeWire test: connect/disconnect sessions, play/stop, idle > grace, no cross-thread PipeWire errors.
- [ ] Socket deployment uses backend-enabled binary, not stale `/opt` noop build.
- [ ] `plan-coolup-cooldown.md` reviewed and updated after channel refactor lands.

---

## Open decisions

1. **Disconnect semantics on warm-down:** should idle reaper fully tear down the PipeWire stream (mainloop thread) or merely pause/deactivate? Full teardown saves resources but increases reconnect complexity/risk.
2. **Leaked session policy:** add session lease/heartbeat/timeout, or accept best-effort disconnect with operational monitoring?
3. **Where to track this plan:** `~/git/l337-audio-server/plan-coolup-cooldown.md` (this file) vs player repo vs both. Audio-server root matches existing plan conventions.
4. **Commit strategy:** keep session API/reaper in one commit and PipeWire channel refactor in a follow-up, or split by subsystem. Separate is safer given PipeWire risk.
