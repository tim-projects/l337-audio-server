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

// pa_context_state_t
const PA_CONTEXT_READY: c_int = 4;
// pa_stream_state_t
const PA_STREAM_READY: c_int = 2;
const PA_STREAM_FAILED: c_int = 3;

// pa_stream_flags_t subset used for playback
const PA_STREAM_INTERPOLATE_TIMING: u32 = 0x0002;
const PA_STREAM_AUTO_TIMING_UPDATE: u32 = 0x0008;
const PA_STREAM_ADJUST_LATENCY: u32 = 0x0100;

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

    fn teardown(&self) {
        unsafe {
            if !self.stream.is_null() {
                pa_stream_disconnect(self.stream);
                pa_stream_unref(self.stream);
            }
            if !self.context.is_null() {
                pa_context_disconnect(self.context);
                pa_context_unref(self.context);
            }
            if !self.mainloop.is_null() {
                pa_threaded_mainloop_stop(self.mainloop);
                pa_threaded_mainloop_free(self.mainloop);
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
    fn writer_loop(state: Arc<PulseAudioState>) {
        const CHUNK_FRAMES: usize = 256;
        let silence = vec![0u8; CHUNK_FRAMES * 4];

        loop {
            if state.cancel.load(Ordering::Relaxed) || state.backend_error.load(Ordering::SeqCst) {
                break;
            }

            // Bail out if the buffer is over budget (mirrors the AAudio/ALSA paths).
            {
                let mut buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
                if buf.pcm.len() > buf.max_bytes / 4 {
                    buf.pcm.clear();
                    buf.read_pos = 0;
                    buf.backend_error.store(true, Ordering::SeqCst);
                    drop(buf);
                    tracing::warn!("PulseAudio buffer cap exceeded, signalling backend error");
                    break;
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
            let outcome: Result<bool, String> = (|| {
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
                    drop(buf);
                    // pa_stream_write with a NULL free callback copies the data.
                    if unsafe {
                        pa_stream_write(stream_ptr, data.as_ptr() as *const c_void, data.len(), std::ptr::null(), 0, PA_SEEK_RELATIVE)
                    } < 0
                    {
                        return Err("pa_stream_write failed".to_string());
                    }
                    Ok(false)
                } else if !playing {
                    drop(buf);
                    // Keep the device awake with silence while paused.
                    if unsafe {
                        pa_stream_write(stream_ptr, silence.as_ptr() as *const c_void, silence.len(), std::ptr::null(), 0, PA_SEEK_RELATIVE)
                    } < 0
                    {
                        return Err("pa_stream_write (silence) failed".to_string());
                    }
                    Ok(true)
                } else {
                    drop(buf);
                    Ok(true)
                }
            })();

            // Never sleep while holding the mainloop lock: the PA mainloop
            // thread needs it to process its iterations.
            handle.unlock();
            drop(handle);

            match outcome {
                Ok(true) => thread::sleep(Duration::from_millis(10)),
                Ok(false) => {}
                Err(e) => {
                    state.backend_error.store(true, Ordering::SeqCst);
                    tracing::warn!("PulseAudio {}, signalling backend error", e);
                    break;
                }
            }
        }
    }
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

            let flags = PA_STREAM_INTERPOLATE_TIMING | PA_STREAM_AUTO_TIMING_UPDATE | PA_STREAM_ADJUST_LATENCY;
            if pa_stream_connect_playback(
                stream,
                std::ptr::null(),
                std::ptr::null(),
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
                PulseAudioAudioOutputStream::writer_loop(state_clone);
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

        let handle = match self.state.handle.lock() {
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
