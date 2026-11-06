use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};

// PulseAudio's client library must be linked at build time (see build.rs and
// plan-android.md sections 4.6/4.8/4.9): a dlopen'd libpulse fails client-side
// with PA_ERR_INVALID, while direct calls from a linked binary work.
//
// We deliberately drive the pa_threaded_mainloop / pa_context / pa_stream API
// instead of the pa_simple_* convenience wrapper: the Termux build of
// libpulse-simple.so (17.0-dirty) has a patched pa_simple_new that returns
// PA_ERR_INVALID inside this process, while the equivalent documented flow
// below succeeds in the very same process (verified with an LD_PRELOAD
// reimplementation: context READY, stream READY, "Using PulseAudio backend").

#[repr(C)]
struct pa_sample_spec {
    format: c_int,
    // Field order must match PulseAudio's pa_sample_spec exactly:
    // { format, rate, channels }. Declaring channels before rate swaps the
    // two values at the ABI boundary and makes pa_stream_new fail.
    rate: u32,
    channels: u8,
}

const PA_SAMPLE_FLOAT32LE: c_int = 5;

/// PulseAudio's pa_buffer_attr; field order and width must match the C ABI.
/// `u32::MAX` in a field means "-1" to PulseAudio (use the default).
#[repr(C)]
struct pa_buffer_attr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: i32,
}

// pa_context_state_t
const PA_CONTEXT_READY: c_int = 4;
// pa_stream_state_t
const PA_STREAM_READY: c_int = 2;
const PA_STREAM_FAILED: c_int = 3;

// pa_stream_flags_t subset used for playback
const PA_STREAM_INTERPOLATE_TIMING: u32 = 0x0002;
const PA_STREAM_AUTO_TIMING_UPDATE: u32 = 0x0008;

/// Target sink-side buffer, in milliseconds.
///
/// A ~100 ms delay is inaudible for music and works like a jitter buffer: it
/// absorbs short writer stalls (the device report of whole-process freezes,
/// and ordinary scheduler jitter) that a just-in-time writer would otherwise
/// pass straight to the sink as a drop. PulseAudio tunes this *down* to
/// whatever the device allows unless the stream asks for a specific latency,
/// and it overrides the request while PA_STREAM_ADJUST_LATENCY is set — so the
/// request has to be explicit and that flag must go. maxlength stays bounded
/// (see the buffer_attr below) so the backlog can never grow without limit
/// (the BUG-2 property).
const PULSE_SINK_TARGET_MSECS: u32 = 60;

// pa_seek_mode_t
const PA_SEEK_RELATIVE: c_int = 0;

unsafe extern "C" {
    fn pa_threaded_mainloop_new() -> *mut c_void;
    fn pa_threaded_mainloop_free(m: *mut c_void);
    fn pa_threaded_mainloop_get_api(m: *mut c_void) -> *mut c_void;
    fn pa_threaded_mainloop_start(m: *mut c_void) -> c_int;
    fn pa_threaded_mainloop_stop(m: *mut c_void);
    fn pa_threaded_mainloop_lock(m: *mut c_void) -> c_int;
    fn pa_threaded_mainloop_unlock(m: *mut c_void) -> c_int;
    // NOTE: two arguments (m, wait_for_accept). Passing garbage for the second
    // one makes the caller block forever in pa_threaded_mainloop_accept.
    fn pa_threaded_mainloop_signal(m: *mut c_void, wait_for_accept: c_int);
    fn pa_threaded_mainloop_wait(m: *mut c_void);

    fn pa_context_new(mainloop_api: *mut c_void, name: *const c_char) -> *mut c_void;
    fn pa_context_unref(c: *mut c_void);
    fn pa_context_set_state_callback(
        c: *mut c_void,
        cb: Option<extern "C" fn(*mut c_void, *mut c_void)>,
        userdata: *mut c_void,
    );
    fn pa_context_connect(
        c: *mut c_void,
        server: *const c_char,
        flags: c_int,
        api: *const c_void,
    ) -> c_int;
    fn pa_context_disconnect(c: *mut c_void);
    fn pa_context_get_state(c: *mut c_void) -> c_int;
    fn pa_context_errno(c: *mut c_void) -> c_int;

    fn pa_stream_new(
        c: *mut c_void,
        name: *const c_char,
        ss: *const pa_sample_spec,
        map: *const c_void,
    ) -> *mut c_void;
    fn pa_stream_unref(s: *mut c_void);
    fn pa_stream_set_state_callback(
        s: *mut c_void,
        cb: Option<extern "C" fn(*mut c_void, *mut c_void)>,
        userdata: *mut c_void,
    );
    fn pa_stream_connect_playback(
        s: *mut c_void,
        dev: *const c_char,
        attr: *const c_void,
        flags: u32,
        volume: *const c_void,
        sync_stream: *mut c_void,
    ) -> c_int;
    fn pa_stream_write(
        s: *mut c_void,
        data: *const c_void,
        nbytes: usize,
        free_cb: *const c_void,
        offset: i64,
        // pa_seek_mode_t: PA_SEEK_RELATIVE = 0. Mandatory sixth argument —
        // declaring five passes register garbage and fails with PA_ERR_INVALID.
        seek: c_int,
    ) -> c_int;
    fn pa_stream_disconnect(s: *mut c_void) -> c_int;
    fn pa_stream_get_state(s: *mut c_void) -> c_int;

    fn pa_strerror(error: c_int) -> *const c_char;
}

fn pa_error_string(error: c_int) -> String {
    unsafe {
        let ptr = pa_strerror(error);
        if ptr.is_null() {
            return format!("error {}", error);
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

// State callbacks: signal the mainloop only for terminal states, mirroring
// PulseAudio's own context_state_cb/stream_state_cb. `userdata` is the
// pa_threaded_mainloop we must wake.
extern "C" fn context_state_cb(context: *mut c_void, userdata: *mut c_void) {
    let state = unsafe { pa_context_get_state(context) };
    // PA_CONTEXT_READY = 4, PA_CONTEXT_FAILED = 5, PA_CONTEXT_TERMINATED = 6
    if state == PA_CONTEXT_READY || state >= 5 {
        unsafe { pa_threaded_mainloop_signal(userdata, 0) };
    }
}

extern "C" fn stream_state_cb(stream: *mut c_void, userdata: *mut c_void) {
    let state = unsafe { pa_stream_get_state(stream) };
    // PA_STREAM_READY = 2, PA_STREAM_FAILED = 3, terminating/terminated follow
    if state == PA_STREAM_READY || state >= PA_STREAM_FAILED {
        unsafe { pa_threaded_mainloop_signal(userdata, 0) };
    }
}

/// Owns the live PulseAudio objects. Every access happens with the mainloop
/// lock held; the writer thread takes that same lock per chunk.
struct PulseAudioHandle {
    mainloop: *mut c_void,
    context: *mut c_void,
    stream: *mut c_void,
}

unsafe impl Send for PulseAudioHandle {}
unsafe impl Sync for PulseAudioHandle {}

impl PulseAudioHandle {
    fn lock(&self) -> c_int {
        unsafe { pa_threaded_mainloop_lock(self.mainloop) }
    }

    fn unlock(&self) -> c_int {
        unsafe { pa_threaded_mainloop_unlock(self.mainloop) }
    }

    /// Idempotent teardown: frees each PulseAudio object at most once. The
    /// writer thread must already be joined (see PulseAudioAudioOutputStream::stop)
    /// before this runs — it races the PA ref-count assertion otherwise. Nulling
    /// the pointers after free makes a second teardown (Drop after try_recover)
    /// a no-op instead of a use-after-free/abort (G5).
    fn teardown(&mut self) {
        unsafe {
            if !self.stream.is_null() {
                pa_stream_disconnect(self.stream);
                pa_stream_unref(self.stream);
                self.stream = std::ptr::null_mut();
            }
            if !self.context.is_null() {
                pa_context_disconnect(self.context);
                pa_context_unref(self.context);
                self.context = std::ptr::null_mut();
            }
            if !self.mainloop.is_null() {
                pa_threaded_mainloop_stop(self.mainloop);
                pa_threaded_mainloop_free(self.mainloop);
                self.mainloop = std::ptr::null_mut();
            }
        }
    }
}

struct PulseAudioState {
    buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    playing: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    backend_error: Arc<AtomicBool>,
    handle: Mutex<PulseAudioHandle>,
}

pub struct PulseAudioAudioBackend;

pub struct PulseAudioAudioOutputStream {
    state: Arc<PulseAudioState>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl PulseAudioAudioOutputStream {
    fn writer_loop(state: Arc<PulseAudioState>, rate: u32, channels: u16) {
        // Consecutive starved polls during playback. One poll (~10 ms) is
        // inside the sink's own buffer and inaudible; a sustained drain is a
        // real drop, and this also skips the end-of-track tail (the writer
        // drains the last samples for a poll or two before the state flips).
        let starved_polls = std::sync::atomic::AtomicU32::new(0);
        let starved_logged = std::sync::atomic::AtomicBool::new(false);
        const CHUNK_FRAMES: usize = 4096;
        let silence = vec![0u8; CHUNK_FRAMES * 4];
        // Pace from the parameters this stream was *created* with, never from the
        // shared buffer: the buffer's `channels` is 0 before the engine fills it
        // and can disagree with the negotiated sample spec. A 2026-11-06 device
        // report traced audible drops to exactly that — a 2ch stream paced with
        // a 1ch assumption wrote 42.7 ms of audio every 85 ms, so the sink's
        // ~43 ms buffer ran dry twice per chunk.
        let rate = rate.max(1);
        let channels = channels.max(1);

        loop {
            if state.cancel.load(Ordering::Relaxed) || state.backend_error.load(Ordering::SeqCst) {
                break;
            }

            // G4: the rebuffer window is bounded by the streaming *producer*
            // throttle (throttle_stream_producer) and by this writer's paced
            // drain + reclaim, so a buffer over the window (e.g. a finite decoded
            // track at t=0) just drains — it is a diagnostic, never a fatal error.
            {
                let buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
                if buf.enters_over_window() {
                    tracing::debug!("PulseAudio audio buffer over rebuffer window; draining (paced)");
                }
            }

            let playing = state.playing.load(Ordering::Relaxed);
            let handle = match state.handle.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            let lock_result = handle.lock();

            if lock_result < 0 {
                drop(handle);
                state.backend_error.store(true, Ordering::SeqCst);
                tracing::warn!("PulseAudio mainloop lock failed, signalling backend error");
                break;
            }

            let stream_ptr = handle.stream;
            // Returns frames of audio written this iteration (0 = silence/starved),
            // so the writer can pace itself to the device clock (G4).
            let outcome: Result<usize, String> = (|| {
                let mut buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
                let available = buf.pcm.len().saturating_sub(buf.read_pos);

                if available > 0 {
                    let to_copy = available.min(CHUNK_FRAMES);
                    let volume = *state.volume.lock().unwrap_or_else(|e| e.into_inner());
                    let mut samples = vec![0.0f32; to_copy];
                    for (dst, src) in samples
                        .iter_mut()
                        .zip(buf.pcm[buf.read_pos..buf.read_pos + to_copy].iter())
                    {
                        *dst = src * volume;
                    }
                    let data = unsafe { samples.align_to::<u8>().1 };
                    buf.read_pos += to_copy;
                    // Reclaim consumed samples at a checkpoint so the Vec shrinks
                    // (keep memory bounded, independent of track length — G4).
                    if buf.read_pos >= (1 << 20) {
                        buf.reclaim_consumed();
                    }
                    drop(buf);
                    // pa_stream_write with a NULL free callback copies the data.
                    if unsafe {
                        pa_stream_write(stream_ptr, data.as_ptr() as *const c_void, data.len(), std::ptr::null(), 0, PA_SEEK_RELATIVE)
                    } < 0
                    {
                        return Err("pa_stream_write failed".to_string());
                    }
                    Ok(to_copy as usize)
                } else if !playing {
                    drop(buf);
                    // Keep the device awake with silence while paused.
                    if unsafe {
                        pa_stream_write(stream_ptr, silence.as_ptr() as *const c_void, silence.len(), std::ptr::null(), 0, PA_SEEK_RELATIVE)
                    } < 0
                    {
                        return Err("pa_stream_write (silence) failed".to_string());
                    }
                    // Report the samples written so the caller paces this like
                    // audio: an unpaced keep-alive writes ~8x real time and piles
                    // a ~1000 s backlog into the sink-input, which is what made
                    // a *paused* stream look like the unbounded-buffer bug.
                    Ok(silence.len() / 4)
                } else {
                    drop(buf);
                    // Edge-triggered: the writer had nothing to write while
                    // playback is running, so the sink's buffer ran dry. One
                    // diagnostic per episode (a device report showed the pacing
                    // reading the buffer's channel count and starving twice per
                    // chunk; 60 lines/s made that invisible, one per episode
                    // does not).
                    let polls = starved_polls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if polls >= 3 && !starved_logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        tracing::warn!(
                            "audio writer starved while playing for ~{}ms: sink buffer ran dry",
                            polls * 10
                        );
                    }
                    Ok(0)
                }
            })();

            // Never sleep while holding the mainloop lock: the PA mainloop
            // thread needs it to process its iterations.
            handle.unlock();
            drop(handle);

            match outcome {
                Ok(n) if n > 0 => {
                    // Writing again ends any starvation episode.
                    starved_polls.store(0, std::sync::atomic::Ordering::Relaxed);
                    starved_logged.store(false, std::sync::atomic::Ordering::Relaxed);
                    // Pace to the device clock: read_pos now advances at ~1x (G4).
                    thread::sleep(Duration::from_secs_f64(pace_seconds(n, rate, channels)));
                }
                Ok(_) => thread::sleep(Duration::from_millis(10)),
                Err(e) => {
                    state.backend_error.store(true, Ordering::SeqCst);
                    tracing::warn!("PulseAudio {}, signalling backend error", e);
                    break;
                }
            }
        }
    }
}


/// How long the writer must sleep after writing *samples* interleaved samples,
/// so the sink receives audio at exactly 1x. `channels`/`rate` must be the
/// parameters the stream was created with: using a channel count that disagrees
/// with the stream's sample spec scales the delivery rate — the 2026-11-06
/// device report was a 2ch stream paced as mono, i.e. 85 ms sleeps for 42 ms
/// chunks, and the sink's small buffer ran dry (continuous audible drops).
fn pace_seconds(samples: usize, rate: u32, channels: u16) -> f64 {
    samples as f64 / (rate.max(1) as f64 * channels.max(1) as f64)
}

impl AudioBackend for PulseAudioAudioBackend {
    fn start_stream(
        &self,
        _name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        let name = CString::new("l337-audio-server").unwrap();
        let stream_name = CString::new("Audio Output").unwrap();

        let ss = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            channels: channels as u8,
            rate: sample_rate,
        };

        unsafe {
            let mainloop = pa_threaded_mainloop_new();
            if mainloop.is_null() {
                return Err("pa_threaded_mainloop_new returned null".to_string());
            }

            let api = pa_threaded_mainloop_get_api(mainloop);
            if api.is_null() {
                pa_threaded_mainloop_free(mainloop);
                return Err("pa_threaded_mainloop_get_api returned null".to_string());
            }

            let context = pa_context_new(api, name.as_ptr());
            if context.is_null() {
                pa_threaded_mainloop_free(mainloop);
                return Err("pa_context_new returned null".to_string());
            }

            pa_context_set_state_callback(context, Some(context_state_cb), mainloop);

            if pa_context_connect(context, std::ptr::null(), 0, std::ptr::null()) < 0 {
                let err = pa_context_errno(context);
                pa_context_unref(context);
                pa_threaded_mainloop_free(mainloop);
                return Err(format!("pa_context_connect failed: {}", pa_error_string(err)));
            }

            if pa_threaded_mainloop_lock(mainloop) < 0 {
                pa_context_disconnect(context);
                pa_context_unref(context);
                pa_threaded_mainloop_free(mainloop);
                return Err("pa_threaded_mainloop_lock failed".to_string());
            }

            if pa_threaded_mainloop_start(mainloop) < 0 {
                pa_threaded_mainloop_unlock(mainloop);
                pa_context_disconnect(context);
                pa_context_unref(context);
                pa_threaded_mainloop_free(mainloop);
                return Err("pa_threaded_mainloop_start failed".to_string());
            }

            // The mainloop thread drives the connection; wait for READY.
            loop {
                let state = pa_context_get_state(context);
                if state == PA_CONTEXT_READY {
                    break;
                }
                // Transient states: 1 CONNECTING, 2 AUTHORIZING, 3 SETTING_NAME.
                if state != 1 && state != 2 && state != 3 {
                    let err = pa_context_errno(context);
                    pa_threaded_mainloop_unlock(mainloop);
                    pa_context_disconnect(context);
                    pa_context_unref(context);
                    pa_threaded_mainloop_free(mainloop);
                    return Err(format!(
                        "context entered state {}: {}",
                        state,
                        pa_error_string(err)
                    ));
                }
                pa_threaded_mainloop_wait(mainloop);
            }

            let stream = pa_stream_new(context, stream_name.as_ptr(), &ss, std::ptr::null());
            if stream.is_null() {
                let err = pa_context_errno(context);
                pa_threaded_mainloop_unlock(mainloop);
                pa_context_disconnect(context);
                pa_context_unref(context);
                pa_threaded_mainloop_free(mainloop);
                return Err(format!("pa_stream_new failed: {}", pa_error_string(err)));
            }

            pa_stream_set_state_callback(stream, Some(stream_state_cb), mainloop);

            // Ask for the target latency rather than accepting the ~43 ms PA
            // default, and without PA_STREAM_ADJUST_LATENCY PA keeps what we
            // asked for. maxlength allows headroom for timing jitter but still
            // bounds the backlog; prebuf = -1 lets PA use tlength, so playback
            // waits the target delay before the first sample (a ~100 ms start,
            // inaudible) and then keeps that margin full.
            let bytes_per_msec = (sample_rate as u32 * channels as u32 * 4) / 1000;
            let tlength = bytes_per_msec * PULSE_SINK_TARGET_MSECS;
            let buffer_attr = pa_buffer_attr {
                maxlength: tlength * 2,
                tlength,
                prebuf: u32::MAX,
                minreq: u32::MAX,
                fragsize: u32::MAX as i32,
            };
            let flags = PA_STREAM_INTERPOLATE_TIMING | PA_STREAM_AUTO_TIMING_UPDATE;
            if pa_stream_connect_playback(
                stream,
                std::ptr::null(),
                &buffer_attr as *const pa_buffer_attr as *const c_void,
                flags,
                std::ptr::null(),
                std::ptr::null_mut(),
            ) < 0
            {
                pa_stream_unref(stream);
                pa_threaded_mainloop_unlock(mainloop);
                pa_context_disconnect(context);
                pa_context_unref(context);
                pa_threaded_mainloop_free(mainloop);
                return Err("pa_stream_connect_playback failed".to_string());
            }

            // Transient stream states: 0 UNCONNECTED, 1 CREATING.
            loop {
                let state = pa_stream_get_state(stream);
                if state == PA_STREAM_READY {
                    break;
                }
                if state != 0 && state != 1 {
                    pa_stream_unref(stream);
                    pa_threaded_mainloop_unlock(mainloop);
                    pa_context_disconnect(context);
                    pa_context_unref(context);
                    pa_threaded_mainloop_free(mainloop);
                    return Err(format!("stream entered state {}", state));
                }
                pa_threaded_mainloop_wait(mainloop);
            }

            pa_threaded_mainloop_unlock(mainloop);

            let state = Arc::new(PulseAudioState {
                buffer: audio_buffer,
                volume,
                playing: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                backend_error: Arc::new(AtomicBool::new(false)),
                handle: Mutex::new(PulseAudioHandle {
                    mainloop,
                    context,
                    stream,
                }),
            });

            let state_clone = state.clone();
            let thread = thread::spawn(move || {
                PulseAudioAudioOutputStream::writer_loop(state_clone, sample_rate, channels);
            });

            Ok(Box::new(PulseAudioAudioOutputStream {
                state,
                thread: Mutex::new(Some(thread)),
            }))
        }
    }
}

impl AudioOutputStream for PulseAudioAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.state.backend_error.store(false, Ordering::SeqCst);
        self.state.playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.state.backend_error.store(false, Ordering::SeqCst);
        self.state.playing.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn stop(&mut self) {
        self.state.backend_error.store(false, Ordering::SeqCst);
        self.state.cancel.store(true, Ordering::Relaxed);

        let thread = match self.thread.lock() {
            Ok(mut guard) => guard.take(),
            Err(e) => e.into_inner().take(),
        };

        if let Some(thread) = thread {
            let _ = thread.join();
        }

        let mut handle = match self.state.handle.lock() {
            Ok(guard) => guard,
            Err(e) => e.into_inner(),
        };
        handle.teardown();
    }
}

impl Drop for PulseAudioAudioOutputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod pacing_tests {
    use super::pace_seconds;

    #[test]
    fn stereo_chunk_paces_to_its_real_duration() {
        // CHUNK_FRAMES is 4096 *samples*; at 48 kHz stereo that is 2048 frames.
        let secs = pace_seconds(4096, 48000, 2);
        assert!(
            (secs - 4096.0 / (48000.0 * 2.0)).abs() < 1e-9,
            "pace must be samples over frames-per-second: {secs}"
        );
    }

    #[test]
    fn a_wrong_channel_count_doubles_the_pace() {
        // The device bug: pacing a 2ch stream as if it were mono sleeps twice
        // as long as the audio written, so the sink starves every other chunk.
        let correct = pace_seconds(4096, 48000, 2);
        let wrong = pace_seconds(4096, 48000, 1);
        assert!((wrong / correct - 2.0).abs() < 1e-9, "mono pacing must be 2x stereo");
        assert!((0.085..0.086).contains(&wrong), "the observed 85 ms gap: {wrong}");
    }

    #[test]
    fn mono_and_high_rates_stay_consistent() {
        assert!((pace_seconds(48000, 48000, 1) - 1.0).abs() < 1e-9);
        assert!((pace_seconds(96000, 48000, 1) - 2.0).abs() < 1e-9);
        // Degenerate channel/rate values must not divide by zero.
        assert!(pace_seconds(1024, 0, 0).is_finite());
    }
}
