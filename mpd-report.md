# MPD PipeWire Analysis — Report

Source: MPD repo cloned to `/tmp/mpd-repo`, files:
- `src/output/plugins/PipeWireOutputPlugin.cxx` (1067 lines)
- `src/output/plugins/PipeWireOutputPlugin.hxx`
- `src/util/RingBuffer.hxx` (332 lines)
- `src/lib/pipewire/ThreadLoop.hxx`

## Key Architectural Differences from l337-audio-server

### 1. Lock-free RingBuffer between producer and consumer (THE BIG ONE)

MPD uses a single-producer/single-consumer **lock-free RingBuffer** (`src/util/RingBuffer.hxx`) to pass PCM data from `Play()` (producer thread) to `Process()` (PipeWire RT callback). The RingBuffer uses `std::atomic_size_t` read/write positions — **no mutex, no blocking** in either direction.

| | MPD | l337-audio-server |
|---|---|---|
| Producer→consumer queue | Lock-free `RingBuffer<std::byte>` | `Arc<Mutex<AudioBuffer>>` with `pcm: Vec<f32>` + `read_pos` |
| RT callback locking | **None** — lock-free RingBuffer reads | `ab.lock()` held while copying PCM |
| Producer blocking | `pw_thread_loop_wait()` when ring full | `wait_for_space()` condvar (20ms) |
| Consumer underrun | Fills PipeWire buffer with **silence**, returns full `chunk.size` | Returns early / `chunk.size = 0` → gap |

The RingBuffer is 0.5 seconds of audio (`sample_rate / 2` frames, capped at 16 MiB). `Play()` writes into it; `Process()` reads out of it.

### 2. Silence on underrun — no gaps ever

MPD's `Process()` (lines 809-877) **always** dequeues a buffer, **always** fills it, and **always** queues it back:

```cpp
size_t nbytes = ring_buffer.ReadFramesTo(dest, chunk_size);
if (nbytes == 0) {
    /* buffer underrun: generate some silence */
    nbytes = max_chunks * chunk_size;
    PcmSilence(dest.first(nbytes), sample_format);
    silence_inserted.store(true, ...);  // logged by producer thread, NOT RT
}
chunk.size = nbytes;   // exact bytes written — never 0 unless truly no buffer
pw_stream_queue_buffer(stream, b);
```

If the ring buffer is empty, PipeWire gets a full buffer of **silence** — the playhead advances continuously, no xrun, no gap. The producer thread logs "Decoder is too slow" on a 5-second throttle.

**Our code does the opposite:** when `available == 0`, we `return` early without queuing a buffer (gap), or set `chunk.size = 0` (stutter/silence glitch). When `to_copy < f32_len`, we set `chunk.size = to_copy * 4` (partial fill → PipeWire plays stale data in the remainder).

### 3. Stream starts INACTIVE, pre-filled before activation

MPD connects with `PW_STREAM_FLAG_INACTIVE` (line 638). `Play()` only calls `pw_stream_set_active(stream, true)` once the ring buffer has enough data (≥ ~0.5s). This guarantees the RT callback never starves at startup.

**Our code** activates immediately via `stream.play()` before decode fills the buffer, causing initial underrun.

### 4. No heap allocation in RT callback

MPD's `Process()` does zero heap allocation — it writes directly into the dequeued PipeWire buffer via `ReadFramesTo`. Our previous code did `Vec::collect()` inside the RT callback (fixed in last version, but the pattern of allocating/copying under lock remains conceptually similar).

PipeWire docs explicitly state: *"You MUST use RT safe functions in the process callback... no allocation or file access or any locking."*

### 5. Volume via PipeWire stream controls, not sample multiplication in RT

MPD sets volume via `pw_stream_set_control(stream, SPA_PROP_channelVolumes, ...)` (line 359). The RT callback copies raw PCM — no per-sample multiplication.

**Our code** multiplies every sample by volume in the RT callback (`*d = s * v`), adding per-sample work to the real-time path.

### 6. Thread model: dedicated pw_thread_loop + producer wait/signal

MPD creates a `pw_thread_loop_new` / `pw_thread_loop_start` — a dedicated real-time thread running `main_loop.run()`. The `Process()` callback runs on this RT thread.

The producer thread (`Play()`) writes to the RingBuffer, then blocks on `pw_thread_loop_wait(thread_loop)` when the ring is full. The RT callback calls `pw_thread_loop_signal(thread_loop, false)` after consuming (line 876) to wake the producer.

**Our code** uses `std::thread::spawn` for the PipeWire main loop and a `Mutex<AudioBuffer>` shared with the decode task — no dedicated RT thread, no producer wait/signal coordination.

### 7. Always queue buffer, even on error

MPD's `Process()` queues the buffer back in every path (lines 837, 874). Even if `d.data == nullptr`, it sets `chunk.size = 0` and queues (line 836-838). Our code returns early without queuing in several paths.

### 8. `pw_stream_flush` for drain

MPD uses `pw_stream_flush(stream, true)` to drain on track end (line 815), ensuring the last audio plays out before state changes to stopped.

---

## Specific Mistakes in l337-audio-server's `src/platform/linux.rs`

1. **Mutex held in RT callback** — `ab.lock()` blocks the real-time thread while the decode task may be waiting on `wait_for_space()`. This is the primary cause of stuttering: the RT thread can't consume while the producer is blocked, and vice versa.

2. **Early return without queueing buffer** — when `available == 0`, we `return` without calling `stream.dequeue_buffer()` + `queue_buffer()`. PipeWire gets no buffer for that cycle → xrun/gap.

3. **`chunk.size = 0` on empty** — tells PipeWire to advance playhead with no audio → silence glitch.

4. **`chunk.size = to_copy * 4` on partial fill** — PipeWire plays `to_copy` samples of new audio, then whatever garbage was in the rest of the buffer → noise/stutter.

5. **`reclaim_consumed()` in RT callback** (earlier version) — `pcm.drain(0..rp)` does a memmove in the real-time thread → missed deadlines → xrun. Removed in current version but the Mutex+copy pattern remains.

6. **Per-sample volume multiplication in RT callback** — unnecessary CPU work on the real-time thread.

---

## Recommended Fix for l337-audio-server

Replace `Arc<Mutex<AudioBuffer>>` shared-buffer model with a **lock-free RingBuffer** (like MPD's `src/util/RingBuffer.hxx`):

1. **Decode/producer thread**: writes PCM frames into RingBuffer; blocks (`wait_for_space` or equivalent) when full.
2. **PipeWire RT callback**: reads frames from RingBuffer (lock-free); if empty, fills buffer with silence; always queues buffer back; sets `chunk.size` to exact bytes written.
3. **Stream starts inactive**; activate only after RingBuffer has ≥ ~0.5s pre-filled.
4. **Volume**: set via PipeWire stream controls, not per-sample in RT callback.
5. **`pw_thread_loop_signal`** from RT callback to wake producer after consuming.
6. **No `Vec::collect()` or any allocation** in RT callback.
7. **`pw_stream_flush(stream, true)`** on track end for clean drain.

This matches the PipeWire documentation's explicit requirement: *"Sound hardware will xrun if there is any delay in audio processing"* — the RT callback must never block, never allocate, and must always return a complete buffer.