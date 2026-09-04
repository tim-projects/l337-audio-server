use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};

mod ffi {
    #[repr(C)]
    pub struct pa_simple {
        _private: [u8; 0],
    }

    #[repr(C)]
    pub struct pa_sample_spec {
        pub format: i32,
        pub channels: u8,
        pub rate: u32,
    }

    pub const PA_SAMPLE_FLOAT32LE: i32 = 5;
    pub const PA_DIRECTION_PLAYBACK: i32 = 0;

    #[allow(non_camel_case_types)]
    pub type pa_simple_new_fn = unsafe extern "C" fn(
        server: *const std::os::raw::c_char,
        name: *const std::os::raw::c_char,
        dir: i32,
        dev: *const std::os::raw::c_char,
        stream_name: *const std::os::raw::c_char,
        ss: *const pa_sample_spec,
        map: *const std::os::raw::c_void,
        attr: *const std::os::raw::c_void,
        error: *mut i32,
    ) -> *mut pa_simple;

    #[allow(non_camel_case_types)]
    pub type pa_simple_write_fn = unsafe extern "C" fn(
        s: *mut pa_simple,
        data: *const std::os::raw::c_void,
        bytes: usize,
        error: *mut i32,
    ) -> i32;

    #[allow(non_camel_case_types)]
    pub type pa_simple_drain_fn = unsafe extern "C" fn(
        s: *mut pa_simple,
        error: *mut i32,
    ) -> i32;

    #[allow(non_camel_case_types)]
    pub type pa_simple_free_fn = unsafe extern "C" fn(s: *mut pa_simple);

    #[allow(non_camel_case_types)]
    pub type pa_simple_get_latency_fn = unsafe extern "C" fn(
        s: *mut pa_simple,
        error: *mut i32,
    ) -> u64;
}

#[derive(Clone, Copy)]
struct PulseAudioFuncs {
    pa_simple_new: ffi::pa_simple_new_fn,
    pa_simple_write: ffi::pa_simple_write_fn,
    pa_simple_drain: ffi::pa_simple_drain_fn,
    pa_simple_free: ffi::pa_simple_free_fn,
    pa_simple_get_latency: ffi::pa_simple_get_latency_fn,
}

struct SendSyncPulseAudio(*mut ffi::pa_simple);
unsafe impl Send for SendSyncPulseAudio {}
unsafe impl Sync for SendSyncPulseAudio {}

struct PulseAudioState {
    buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    playing: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    simple: Mutex<SendSyncPulseAudio>,
}

pub struct PulseAudioAudioBackend;

pub struct PulseAudioAudioOutputStream {
    state: Arc<PulseAudioState>,
    funcs: PulseAudioFuncs,
    _lib: Arc<libloading::Library>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl PulseAudioAudioOutputStream {
    fn writer_loop(state: Arc<PulseAudioState>, funcs: PulseAudioFuncs) {
        const CHUNK_FRAMES: usize = 256;
        let silence_buf = vec![0u8; CHUNK_FRAMES * 4];

        loop {
            if state.cancel.load(Ordering::Relaxed) {
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
                    (funcs.pa_simple_write)(
                        simple_ptr,
                        bytes.as_ptr() as *const std::os::raw::c_void,
                        bytes.len(),
                        std::ptr::null_mut(),
                    )
                };

                if result < 0 {
                    break;
                }
            } else if !playing {
                drop(buf);
                let result = unsafe {
                    (funcs.pa_simple_write)(
                        simple_ptr,
                        silence_buf.as_ptr() as *const std::os::raw::c_void,
                        silence_buf.len(),
                        std::ptr::null_mut(),
                    )
                };
                if result < 0 {
                    break;
                }
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
        let lib = Arc::new(
            unsafe {
                libloading::Library::new("libpulse-simple.so.0")
                    .or_else(|_| libloading::Library::new("libpulse-simple.so"))
                    .map_err(|e| format!("Failed to load libpulse-simple: {}", e))?
            },
        );

        let pa_simple_new: libloading::Symbol<ffi::pa_simple_new_fn> = unsafe {
            lib.get(b"pa_simple_new\0")
                .map_err(|e| format!("Failed to load pa_simple_new: {}", e))?
        };
        let pa_simple_write: libloading::Symbol<ffi::pa_simple_write_fn> = unsafe {
            lib.get(b"pa_simple_write\0")
                .map_err(|e| format!("Failed to load pa_simple_write: {}", e))?
        };
        let pa_simple_drain: libloading::Symbol<ffi::pa_simple_drain_fn> = unsafe {
            lib.get(b"pa_simple_drain\0")
                .map_err(|e| format!("Failed to load pa_simple_drain: {}", e))?
        };
        let pa_simple_free: libloading::Symbol<ffi::pa_simple_free_fn> = unsafe {
            lib.get(b"pa_simple_free\0")
                .map_err(|e| format!("Failed to load pa_simple_free: {}", e))?
        };
        let pa_simple_get_latency: libloading::Symbol<ffi::pa_simple_get_latency_fn> = unsafe {
            lib.get(b"pa_simple_get_latency\0")
                .map_err(|e| format!("Failed to load pa_simple_get_latency: {}", e))?
        };

        let funcs = PulseAudioFuncs {
            pa_simple_new: *pa_simple_new,
            pa_simple_write: *pa_simple_write,
            pa_simple_drain: *pa_simple_drain,
            pa_simple_free: *pa_simple_free,
            pa_simple_get_latency: *pa_simple_get_latency,
        };

        let ss = ffi::pa_sample_spec {
            format: ffi::PA_SAMPLE_FLOAT32LE,
            channels: channels as u8,
            rate: sample_rate,
        };

        let mut error = 0;
        let simple = unsafe {
            (funcs.pa_simple_new)(
                std::ptr::null(),
                b"l337-audio-server\0" as *const u8 as *const std::os::raw::c_char,
                ffi::PA_DIRECTION_PLAYBACK,
                std::ptr::null(),
                b"Audio Output\0" as *const u8 as *const std::os::raw::c_char,
                &ss,
                std::ptr::null(),
                std::ptr::null(),
                &mut error,
            )
        };

        if simple.is_null() {
            return Err(format!("pa_simple_new failed with error {}", error));
        }

        let state = Arc::new(PulseAudioState {
            buffer: audio_buffer,
            volume,
            playing: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            simple: Mutex::new(SendSyncPulseAudio(simple)),
        });

        let state_clone = state.clone();
        let thread = thread::spawn(move || {
            PulseAudioAudioOutputStream::writer_loop(state_clone, funcs);
        });

        Ok(Box::new(PulseAudioAudioOutputStream {
            state,
            funcs,
            _lib: lib,
            thread: Mutex::new(Some(thread)),
        }))
    }
}

impl AudioOutputStream for PulseAudioAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.state.playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.state.playing.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn stop(&mut self) {
        self.state.cancel.store(true, Ordering::Relaxed);

        let simple_ptr = match self.state.simple.lock() {
            Ok(guard) => guard.0,
            Err(e) => e.into_inner().0,
        };

        if !simple_ptr.is_null() {
            unsafe {
                (self.funcs.pa_simple_free)(simple_ptr);
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
