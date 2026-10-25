use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};

// PulseAudio's client library must be linked at build time (see build.rs and
// plan-android.md 4.6/4.8): on bionic a dlopen'd libpulse fails client-side in
// pa_stream_new with PA_ERR_INVALID, while direct calls from a linked binary
// work. These declarations resolve at load time against $PREFIX/lib/libpulse*.

#[repr(C)]
struct pa_sample_spec {
    format: c_int,
    // Field order must match PulseAudio's pa_sample_spec exactly:
    // { format, rate, channels }. Declaring channels before rate swaps the
    // two values at the ABI boundary, which sends rate=channels and
    // channels=rate and makes pa_simple_new fail with PA_ERR_INVALID.
    rate: u32,
    channels: u8,
}

const PA_SAMPLE_FLOAT32LE: c_int = 5;
const PA_DIRECTION_PLAYBACK: c_int = 0;

unsafe extern "C" {
    fn pa_simple_new(
        server: *const c_char,
        name: *const c_char,
        dir: c_int,
        dev: *const c_char,
        stream_name: *const c_char,
        ss: *const pa_sample_spec,
        map: *const c_void,
        attr: *const c_void,
        error: *mut c_int,
    ) -> *mut c_void;
    fn pa_simple_write(
        s: *mut c_void,
        data: *const c_void,
        bytes: usize,
        error: *mut c_int,
    ) -> c_int;
    fn pa_simple_drain(s: *mut c_void, error: *mut c_int) -> c_int;
    fn pa_simple_free(s: *mut c_void);
    fn pa_simple_get_latency(s: *mut c_void, error: *mut c_int) -> u64;
    fn pa_strerror(error: c_int) -> *const c_char;
}

/// Render a PA error code as a human-readable string.
fn pa_error_string(error: c_int) -> String {
    unsafe {
        let ptr = pa_strerror(error);
        if ptr.is_null() {
            return format!("error {}", error);
        }
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

struct SendSyncPulseAudio(*mut c_void);
unsafe impl Send for SendSyncPulseAudio {}
unsafe impl Sync for SendSyncPulseAudio {}

struct PulseAudioState {
    buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    playing: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    backend_error: Arc<AtomicBool>,
    simple: Mutex<SendSyncPulseAudio>,
}

pub struct PulseAudioAudioBackend;

pub struct PulseAudioAudioOutputStream {
    state: Arc<PulseAudioState>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl PulseAudioAudioOutputStream {
    fn writer_loop(state: Arc<PulseAudioState>) {
        const CHUNK_FRAMES: usize = 256;
        let silence_buf = vec![0u8; CHUNK_FRAMES * 4];

        loop {
            if state.cancel.load(Ordering::Relaxed) {
                break;
            }

            if state.backend_error.load(Ordering::SeqCst) {
                break;
            }

            let playing = state.playing.load(Ordering::Relaxed);

            let simple_ptr = match state.simple.lock() {
                Ok(guard) => guard.0,
                Err(e) => e.into_inner().0,
            };
            if simple_ptr.is_null() {
                break;
            }

            let mut buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
            if buf.pcm.len() > buf.max_bytes / 4 {
                buf.pcm.clear();
                buf.read_pos = 0;
                buf.backend_error.store(true, Ordering::SeqCst);
                drop(buf);
                tracing::warn!("PulseAudio buffer cap exceeded, signalling backend error");
                break;
            }
            let available = buf.pcm.len().saturating_sub(buf.read_pos);

            if available > 0 {
                let to_copy = available.min(CHUNK_FRAMES);
                let src = &buf.pcm[buf.read_pos..buf.read_pos + to_copy];
                let volume = *state.volume.lock().unwrap_or_else(|e| e.into_inner());

                let mut samples = vec![0.0f32; to_copy];
                for (dst, src) in samples.iter_mut().zip(src.iter()) {
                    *dst = src * volume;
                }

                let bytes = unsafe { samples.align_to::<u8>().1 };
                buf.read_pos += to_copy;
                drop(buf);

                let result = unsafe {
                    pa_simple_write(
                        simple_ptr,
                        bytes.as_ptr() as *const c_void,
                        bytes.len(),
                        std::ptr::null_mut(),
                    )
                };

                if result < 0 {
                    state.backend_error.store(true, Ordering::SeqCst);
                    tracing::warn!("PulseAudio pa_simple_write failed, signalling backend error");
                    break;
                }
            } else if !playing {
                drop(buf);
                let result = unsafe {
                    pa_simple_write(
                        simple_ptr,
                        silence_buf.as_ptr() as *const c_void,
                        silence_buf.len(),
                        std::ptr::null_mut(),
                    )
                };
                if result < 0 {
                    state.backend_error.store(true, Ordering::SeqCst);
                    tracing::warn!("PulseAudio pa_simple_write (silence) failed, signalling backend error");
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            } else {
                drop(buf);
                thread::sleep(Duration::from_millis(10));
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
        let name = c"l337-audio-server";
        let stream_name = c"Audio Output";

        let ss = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            channels: channels as u8,
            rate: sample_rate,
        };

        let mut error: c_int = 0;
        let simple = unsafe {
            pa_simple_new(
                std::ptr::null(),
                name.as_ptr(),
                PA_DIRECTION_PLAYBACK,
                std::ptr::null(),
                stream_name.as_ptr(),
                &ss,
                std::ptr::null(),
                std::ptr::null(),
                &mut error,
            )
        };

        if simple.is_null() {
            return Err(format!("pa_simple_new failed: {}", pa_error_string(error)));
        }

        let state = Arc::new(PulseAudioState {
            buffer: audio_buffer,
            volume,
            playing: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            backend_error: Arc::new(AtomicBool::new(false)),
            simple: Mutex::new(SendSyncPulseAudio(simple)),
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

        let simple_ptr = match self.state.simple.lock() {
            Ok(guard) => guard.0,
            Err(e) => e.into_inner().0,
        };

        if !simple_ptr.is_null() {
            unsafe {
                pa_simple_free(simple_ptr);
            }
        }

        let thread = match self.thread.lock() {
            Ok(mut guard) => guard.take(),
            Err(e) => e.into_inner().take(),
        };

        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl Drop for PulseAudioAudioOutputStream {
    fn drop(&mut self) {
        self.stop();
    }
}
