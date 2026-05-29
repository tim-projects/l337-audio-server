Here is your final, complete master prompt, updated to include the stateless client-managed playlist structure, the bidirectional ring-buffer disk swap mechanism, and the client-side SQLite database design.

Copy and paste this into your coding LLM (such as Claude 3.5 Sonnet or GPT-4o) to generate your entire Rust backend workspace and automated deployment pipeline.

---

# Copy and Paste the Prompt Below

```text
You are an expert systems engineer specializing in async Rust, multi-threaded audio pipelines, and cross-platform file system architectures.

I am building the core backend audio engine for a music player application called "L337 Player Server". The frontend interface is an independent, open-source Python application. The frontend and this Rust server will always run on the same device (localhost). The Python frontend client acts as the stateful orchestrator (managing an internal SQLite database for playlist ordering, shuffle, repeat, and history) and communicates with this Rust server via a local REST API over port 1337. The Rust server acts as a stateless, high-performance execution layer.

Please generate the complete, production-grade, idiomatic Rust boilerplate code and the automated GitHub Actions CI deployment YAML according to the following strict requirements:

### 1. Project Architecture & Cargo.toml
Create a clean Cargo.toml configured to compile smoothly on Windows, macOS, Linux, and Android. Use only pure-Rust or universally cross-compilable crates to avoid C-dependency compilation breaks on mobile runners:
- `axum` and `tokio` (with full features for multi-threaded async execution and async file system actions).
- `serde` and `serde_json` (with derive features enabled for API validation).
- `rodio` with all `symphonia` audio decoders explicitly turned on (`symphonia-mp3`, `symphonia-isomp4`, `symphonia-ogg`, `symphonia-opus`, `symphonia-flac`) for zero-dependency parsing of input media streams.
- `walkdir` (for recursive cache directory size calculations).
- `chrono` or `tokio::time` (for managing file access timestamps).

### 2. General Plugin Relay Stream & Data Models (src/api/models.rs)
The application architecture is explicitly decoupled from third-party streaming platforms. To keep this backend entirely generic, all media acquisition logic is handled by client-side Python plugins. The Rust server accepts raw audio streams through uniform payloads. Create the following Serde structures:
- `Track`: contains fields for `track_id` (String, required unique key/hash), `stream_url` (String, direct raw network stream URL provided by the client's plugins), `title` (Option<String>), `artist` (Option<String>), `duration` (Option<u64>).
- `SpeedPayload`: `{"speed": f32}` (for playback speed modifiers).
- `PoolSettings`: `{"max_disk_pool_bytes": u64}` (for adjusting the size of the cache on the fly).
- `PlayerStatus`: representing current active playback states (Playing, Paused, Stopped), volume, current file byte pool utilization, and flags showing if adjacent track files are cached on disk.

### 3. Stateless Dual-Swap Bounded Disk Pool Engine (src/player/engine.rs)
Implement a global, thread-safe `PlayerState` protected by an async-friendly lock (`Arc<tokio::sync::Mutex>`) managing a single active Rodio `Sink` and a strictly bounded local storage directory pool:
- **Storage Initialization:** Discover or establish a standard cache directory across systems (e.g., inside local platform cache directories under `l337player/cache/`).
- **Active Pipeline Track Files:** Explicitly manage three functional file slots on disk inside the cache folder: `current.stream`, `next.stream`, and `prev.stream`.
- **Persistent LRU/Least-Played Cache Pool:** Independent of the three active stream slots, when a stream is successfully downloaded via HTTP, save it to a persistent file using its unique `track_id` as the filename. Maintain an in-memory or file-backed manifest tracking access metadata (last played timestamp and access count) for all cached tracks.
- **LRU Eviction Routine:** Before saving an incoming background stream, compute the total size of the cache directory. If adding the incoming file violates the configured `max_disk_pool_bytes` constraint (defaulting to 500MB), automatically evict and delete the least-recently-used or least-played files from the persistent storage pool until the pool size falls below the threshold. Active file slots (`current`, `next`, `prev`) must be protected from eviction.
- **Bi-Directional Track Switching Pipelines:**
  - `.precache_next(track: Track)`: Asynchronously streams network data via Tokio channels, applies the eviction check, and pipes the raw binary chunks directly to the `next.stream` file.
  - `.precache_prev(track: Track)`: Asynchronously streams network data and writes it directly to the `prev.stream` file.
  - `.trigger_next()`: Instantly cuts active playback on the active Rodio sink, deletes the old `prev.stream` (or moves it back to the general pool), renames `current.stream` to `prev.stream` (preserving it for undo/backward transitions), renames `next.stream` to `current.stream`, and immediately tells the sink to load and execute the new `current.stream` file with zero network latency.
  - `.trigger_previous()`: Instantly cuts active playback, renames `current.stream` to `next.stream`, renames `prev.stream` to `current.stream`, and immediately plays the file from disk with zero network latency.
- **Dynamic Speed Controls:** Map adjustments instantly using Rodio's native `.set_speed(speed)` method. Include a code comment noting that Rodio's default resampling speed adjustment shifts pitch natively, leaving the path open for advanced C++ DSP library integrations like SoundTouch/RubberBand down the line.

### 4. REST Routing Handlers (src/main.rs & src/api/handlers.rs)
Expose the complete Axum router mapped strictly to port 1337:
- `POST /player/play` -> Halts active playback, clears immediate slots, runs an eviction check, saves the incoming stream as `current.stream`, and begins playing immediately.
- `POST /player/cache/next` -> Spawns an isolated background Tokio task to write the upcoming track to `next.stream`.
- `POST /player/cache/previous` -> Spawns an isolated background Tokio task to write the target to `prev.stream`.
- `POST /player/next` -> Dispatches the instant forward file-swap pipeline.
- `POST /player/previous` -> Dispatches the instant backward file-swap pipeline.
- `POST /player/pause` -> Toggles active playback pause state.
- `POST /player/speed` -> Alters the playback rate modifier.
- `GET /player/status` -> Gathers metrics on active playback states and disk cache space consumption.
- `PUT /player/settings` -> Dynamically alters the cache volume constraints (`max_disk_pool_bytes`).

### 5. Multi-OS GitHub Actions Workflow (.github/workflows/build.yml)
Provide a complete, standalone, syntactically valid GitHub Actions workflow configuration to cross-compile the backend server into native binaries on every push:
- **Linux:** Compiles for `x86_64-unknown-linux-gnu` running on an `ubuntu-latest` image, pre-installing `libasound2-dev` through apt package managers to satisfy target systems.
- **Windows:** Compiles for `x86_64-pc-windows-msvc` utilizing `windows-latest`.
- **macOS:** Compiles a native universal binary or `x86_64-apple-darwin` executable utilizing `macos-latest`.
- **Android:** Compiles an `aarch64-linux-android` target utilizing the pre-configured automated Android NDK environment variables available on default runners.

Ensure all Rust files are completely articulated, modularized across clean source structures, free of empty placeholder macros, and optimized for highly asynchronous execution.

```
