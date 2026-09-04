use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};

mod ffi {
    #![allow(non_camel_case_types)]

    pub type snd_pcm_t = std::os::raw::c_void;
    pub type snd_pcm_hw_params_t = std::os::raw::c_void;
    pub type snd_pcm_sw_params_t = std::os::raw::c_void;

    pub const SND_PCM_STATE_PREPARED: i32 = 2;
    pub const SND_PCM_STATE_RUNNING: i32 = 3;
    pub const SND_PCM_STATE_XRUN: i32 = 4;
    pub const SND_PCM_STATE_SUSPENDED: i32 = 5;

    pub const SND_PCM_ACCESS_RW_INTERLEAVED: i32 = 3;
    pub const SND_PCM_FORMAT_FLOAT_LE: i32 = 14;
    pub const SND_PCM_STREAM_PLAYBACK: i32 = 0;

    pub type snd_pcm_open_fn = unsafe extern "C" fn(
        pcm: *mut *mut snd_pcm_t,
        name: *const std::os::raw::c_char,
        stream: i32,
        mode: i32,
    ) -> i32;

    pub type snd_pcm_close_fn = unsafe extern "C" fn(pcm: *mut snd_pcm_t) -> i32;

    pub type snd_pcm_hw_params_malloc_fn =
        unsafe extern "C" fn(params: *mut *mut snd_pcm_hw_params_t) -> i32;

    pub type snd_pcm_hw_params_free_fn =
        unsafe extern "C" fn(params: *mut snd_pcm_hw_params_t) -> i32;

    pub type snd_pcm_hw_params_any_fn =
        unsafe extern "C" fn(pcm: *mut snd_pcm_t, params: *mut snd_pcm_hw_params_t) -> i32;

    pub type snd_pcm_hw_params_set_access_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_hw_params_t,
            access: i32,
        ) -> i32;

    pub type snd_pcm_hw_params_set_format_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_hw_params_t,
            format: i32,
        ) -> i32;

    pub type snd_pcm_hw_params_set_rate_near_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_hw_params_t,
            rate: *mut u32,
            dir: *mut i32,
        ) -> i32;

    pub type snd_pcm_hw_params_set_channels_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_hw_params_t,
            channels: u32,
        ) -> i32;

    pub type snd_pcm_hw_params_fn =
        unsafe extern "C" fn(pcm: *mut snd_pcm_t, params: *mut snd_pcm_hw_params_t) -> i32;

    pub type snd_pcm_sw_params_malloc_fn =
        unsafe extern "C" fn(params: *mut *mut snd_pcm_sw_params_t) -> i32;

    pub type snd_pcm_sw_params_free_fn =
        unsafe extern "C" fn(params: *mut snd_pcm_sw_params_t) -> i32;

    pub type snd_pcm_sw_params_current_fn =
        unsafe extern "C" fn(pcm: *mut snd_pcm_t, params: *mut snd_pcm_sw_params_t) -> i32;

    pub type snd_pcm_sw_params_set_avail_min_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_sw_params_t,
            val: u64,
        ) -> i32;

    pub type snd_pcm_sw_params_set_start_threshold_fn =
        unsafe extern "C" fn(
            pcm: *mut snd_pcm_t,
            params: *mut snd_pcm_sw_params_t,
            val: u64,
        ) -> i32;

    pub type snd_pcm_sw_params_fn =
        unsafe extern "C" fn(pcm: *mut snd_pcm_t, params: *mut snd_pcm_sw_params_t) -> i32;

    pub type snd_pcm_prepare_fn = unsafe extern "C" fn(pcm: *mut snd_pcm_t) -> i32;

    pub type snd_pcm_writei_fn = unsafe extern "C" fn(
        pcm: *mut snd_pcm_t,
        buffer: *const std::os::raw::c_void,
        size: u64,
    ) -> i64;

    pub type snd_pcm_recover_fn =
        unsafe extern "C" fn(pcm: *mut snd_pcm_t, err: i32, silent: i32) -> i32;

    pub type snd_pcm_state_fn = unsafe extern "C" fn(pcm: *mut snd_pcm_t) -> i32;

    pub type snd_strerror_fn = unsafe extern "C" fn(err: i32) -> *const std::os::raw::c_char;
}

#[derive(Clone, Copy)]
struct AlsaFuncs {
    snd_pcm_open: ffi::snd_pcm_open_fn,
    snd_pcm_close: ffi::snd_pcm_close_fn,
    snd_pcm_hw_params_malloc: ffi::snd_pcm_hw_params_malloc_fn,
    snd_pcm_hw_params_free: ffi::snd_pcm_hw_params_free_fn,
    snd_pcm_hw_params_any: ffi::snd_pcm_hw_params_any_fn,
    snd_pcm_hw_params_set_access: ffi::snd_pcm_hw_params_set_access_fn,
    snd_pcm_hw_params_set_format: ffi::snd_pcm_hw_params_set_format_fn,
    snd_pcm_hw_params_set_rate_near: ffi::snd_pcm_hw_params_set_rate_near_fn,
    snd_pcm_hw_params_set_channels: ffi::snd_pcm_hw_params_set_channels_fn,
    snd_pcm_hw_params: ffi::snd_pcm_hw_params_fn,
    snd_pcm_sw_params_malloc: ffi::snd_pcm_sw_params_malloc_fn,
    snd_pcm_sw_params_free: ffi::snd_pcm_sw_params_free_fn,
    snd_pcm_sw_params_current: ffi::snd_pcm_sw_params_current_fn,
    snd_pcm_sw_params_set_avail_min: ffi::snd_pcm_sw_params_set_avail_min_fn,
    snd_pcm_sw_params_set_start_threshold: ffi::snd_pcm_sw_params_set_start_threshold_fn,
    snd_pcm_sw_params: ffi::snd_pcm_sw_params_fn,
    snd_pcm_prepare: ffi::snd_pcm_prepare_fn,
    snd_pcm_writei: ffi::snd_pcm_writei_fn,
    snd_pcm_recover: ffi::snd_pcm_recover_fn,
    snd_pcm_state: ffi::snd_pcm_state_fn,
    snd_strerror: ffi::snd_strerror_fn,
}

struct SendSyncPcm(*mut ffi::snd_pcm_t);
unsafe impl Send for SendSyncPcm {}
unsafe impl Sync for SendSyncPcm {}

struct AlsaState {
    buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    playing: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    pcm: Mutex<SendSyncPcm>,
}

pub struct AlsaAudioBackend;

pub struct AlsaAudioOutputStream {
    state: Arc<AlsaState>,
    funcs: AlsaFuncs,
    _lib: Arc<libloading::Library>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl AlsaAudioOutputStream {
    fn writer_loop(state: Arc<AlsaState>, funcs: AlsaFuncs) {
        const CHUNK_FRAMES: usize = 256;
        let silence_buf = vec![0u8; CHUNK_FRAMES * 4];

        loop {
            if state.cancel.load(Ordering::Relaxed) {
                break;
            }

            let playing = state.playing.load(Ordering::Relaxed);

            let pcm_ptr = match state.pcm.lock() {
                Ok(guard) => guard.0,
                Err(e) => e.into_inner().0,
            };
            if pcm_ptr.is_null() {
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
                    (funcs.snd_pcm_writei)(
                        pcm_ptr,
                        bytes.as_ptr() as *const std::os::raw::c_void,
                        bytes.len() as u64 / 4,
                    )
                };

                if result < 0 {
                    let recovered = unsafe { (funcs.snd_pcm_recover)(pcm_ptr, result as i32, 1) };
                    if recovered < 0 {
                        break;
                    }
                }
            } else if !playing {
                drop(buf);
                let result = unsafe {
                    (funcs.snd_pcm_writei)(
                        pcm_ptr,
                        silence_buf.as_ptr() as *const std::os::raw::c_void,
                        silence_buf.len() as u64 / 4,
                    )
                };
                if result < 0 {
                    let recovered = unsafe { (funcs.snd_pcm_recover)(pcm_ptr, result as i32, 1) };
                    if recovered < 0 {
                        break;
                    }
                }
            } else {
                drop(buf);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

impl AudioBackend for AlsaAudioBackend {
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
                libloading::Library::new("libasound.so.2")
                    .or_else(|_| libloading::Library::new("libasound.so"))
                    .map_err(|e| format!("Failed to load libasound: {}", e))?
            },
        );

        let snd_pcm_open: libloading::Symbol<ffi::snd_pcm_open_fn> = unsafe {
            lib.get(b"snd_pcm_open\0")
                .map_err(|e| format!("Failed to load snd_pcm_open: {}", e))?
        };
        let snd_pcm_close: libloading::Symbol<ffi::snd_pcm_close_fn> = unsafe {
            lib.get(b"snd_pcm_close\0")
                .map_err(|e| format!("Failed to load snd_pcm_close: {}", e))?
        };
        let snd_pcm_hw_params_malloc: libloading::Symbol<ffi::snd_pcm_hw_params_malloc_fn> = unsafe {
            lib.get(b"snd_pcm_hw_params_malloc\0")
                .map_err(|e| format!("Failed to load snd_pcm_hw_params_malloc: {}", e))?
        };
        let snd_pcm_hw_params_free: libloading::Symbol<ffi::snd_pcm_hw_params_free_fn> = unsafe {
            lib.get(b"snd_pcm_hw_params_free\0")
                .map_err(|e| format!("Failed to load snd_pcm_hw_params_free: {}", e))?
        };
        let snd_pcm_hw_params_any: libloading::Symbol<ffi::snd_pcm_hw_params_any_fn> = unsafe {
            lib.get(b"snd_pcm_hw_params_any\0")
                .map_err(|e| format!("Failed to load snd_pcm_hw_params_any: {}", e))?
        };
        let snd_pcm_hw_params_set_access: libloading::Symbol<ffi::snd_pcm_hw_params_set_access_fn> =
            unsafe {
                lib.get(b"snd_pcm_hw_params_set_access\0")
                    .map_err(|e| format!("Failed to load snd_pcm_hw_params_set_access: {}", e))?
            };
        let snd_pcm_hw_params_set_format: libloading::Symbol<ffi::snd_pcm_hw_params_set_format_fn> =
            unsafe {
                lib.get(b"snd_pcm_hw_params_set_format\0")
                    .map_err(|e| format!("Failed to load snd_pcm_hw_params_set_format: {}", e))?
            };
        let snd_pcm_hw_params_set_rate_near: libloading::Symbol<ffi::snd_pcm_hw_params_set_rate_near_fn> =
            unsafe {
                lib.get(b"snd_pcm_hw_params_set_rate_near\0")
                    .map_err(|e| format!("Failed to load snd_pcm_hw_params_set_rate_near: {}", e))?
            };
        let snd_pcm_hw_params_set_channels: libloading::Symbol<ffi::snd_pcm_hw_params_set_channels_fn> =
            unsafe {
                lib.get(b"snd_pcm_hw_params_set_channels\0")
                    .map_err(|e| format!("Failed to load snd_pcm_hw_params_set_channels: {}", e))?
            };
        let snd_pcm_hw_params: libloading::Symbol<ffi::snd_pcm_hw_params_fn> = unsafe {
            lib.get(b"snd_pcm_hw_params\0")
                .map_err(|e| format!("Failed to load snd_pcm_hw_params: {}", e))?
        };
        let snd_pcm_sw_params_malloc: libloading::Symbol<ffi::snd_pcm_sw_params_malloc_fn> = unsafe {
            lib.get(b"snd_pcm_sw_params_malloc\0")
                .map_err(|e| format!("Failed to load snd_pcm_sw_params_malloc: {}", e))?
        };
        let snd_pcm_sw_params_free: libloading::Symbol<ffi::snd_pcm_sw_params_free_fn> = unsafe {
            lib.get(b"snd_pcm_sw_params_free\0")
                .map_err(|e| format!("Failed to load snd_pcm_sw_params_free: {}", e))?
        };
        let snd_pcm_sw_params_current: libloading::Symbol<ffi::snd_pcm_sw_params_current_fn> =
            unsafe {
                lib.get(b"snd_pcm_sw_params_current\0")
                    .map_err(|e| format!("Failed to load snd_pcm_sw_params_current: {}", e))?
            };
        let snd_pcm_sw_params_set_avail_min: libloading::Symbol<ffi::snd_pcm_sw_params_set_avail_min_fn> =
            unsafe {
                lib.get(b"snd_pcm_sw_params_set_avail_min\0")
                    .map_err(|e| format!("Failed to load snd_pcm_sw_params_set_avail_min: {}", e))?
            };
        let snd_pcm_sw_params_set_start_threshold: libloading::Symbol<ffi::snd_pcm_sw_params_set_start_threshold_fn> =
            unsafe {
                lib.get(b"snd_pcm_sw_params_set_start_threshold\0")
                    .map_err(|e| format!("Failed to load snd_pcm_sw_params_set_start_threshold: {}", e))?
            };
        let snd_pcm_sw_params: libloading::Symbol<ffi::snd_pcm_sw_params_fn> = unsafe {
            lib.get(b"snd_pcm_sw_params\0")
                .map_err(|e| format!("Failed to load snd_pcm_sw_params: {}", e))?
        };
        let snd_pcm_prepare: libloading::Symbol<ffi::snd_pcm_prepare_fn> = unsafe {
            lib.get(b"snd_pcm_prepare\0")
                .map_err(|e| format!("Failed to load snd_pcm_prepare: {}", e))?
        };
        let snd_pcm_writei: libloading::Symbol<ffi::snd_pcm_writei_fn> = unsafe {
            lib.get(b"snd_pcm_writei\0")
                .map_err(|e| format!("Failed to load snd_pcm_writei: {}", e))?
        };
        let snd_pcm_recover: libloading::Symbol<ffi::snd_pcm_recover_fn> = unsafe {
            lib.get(b"snd_pcm_recover\0")
                .map_err(|e| format!("Failed to load snd_pcm_recover: {}", e))?
        };
        let snd_pcm_state: libloading::Symbol<ffi::snd_pcm_state_fn> = unsafe {
            lib.get(b"snd_pcm_state\0")
                .map_err(|e| format!("Failed to load snd_pcm_state: {}", e))?
        };
        let snd_strerror: libloading::Symbol<ffi::snd_strerror_fn> = unsafe {
            lib.get(b"snd_strerror\0")
                .map_err(|e| format!("Failed to load snd_strerror: {}", e))?
        };

        let funcs = AlsaFuncs {
            snd_pcm_open: *snd_pcm_open,
            snd_pcm_close: *snd_pcm_close,
            snd_pcm_hw_params_malloc: *snd_pcm_hw_params_malloc,
            snd_pcm_hw_params_free: *snd_pcm_hw_params_free,
            snd_pcm_hw_params_any: *snd_pcm_hw_params_any,
            snd_pcm_hw_params_set_access: *snd_pcm_hw_params_set_access,
            snd_pcm_hw_params_set_format: *snd_pcm_hw_params_set_format,
            snd_pcm_hw_params_set_rate_near: *snd_pcm_hw_params_set_rate_near,
            snd_pcm_hw_params_set_channels: *snd_pcm_hw_params_set_channels,
            snd_pcm_hw_params: *snd_pcm_hw_params,
            snd_pcm_sw_params_malloc: *snd_pcm_sw_params_malloc,
            snd_pcm_sw_params_free: *snd_pcm_sw_params_free,
            snd_pcm_sw_params_current: *snd_pcm_sw_params_current,
            snd_pcm_sw_params_set_avail_min: *snd_pcm_sw_params_set_avail_min,
            snd_pcm_sw_params_set_start_threshold: *snd_pcm_sw_params_set_start_threshold,
            snd_pcm_sw_params: *snd_pcm_sw_params,
            snd_pcm_prepare: *snd_pcm_prepare,
            snd_pcm_writei: *snd_pcm_writei,
            snd_pcm_recover: *snd_pcm_recover,
            snd_pcm_state: *snd_pcm_state,
            snd_strerror: *snd_strerror,
        };

        let mut pcm: *mut ffi::snd_pcm_t = std::ptr::null_mut();
        let open_result = unsafe {
            (funcs.snd_pcm_open)(
                &mut pcm,
                b"default\0" as *const u8 as *const std::os::raw::c_char,
                ffi::SND_PCM_STREAM_PLAYBACK,
                0,
            )
        };

        if open_result < 0 || pcm.is_null() {
            return Err(format!(
                "snd_pcm_open failed: {}",
                unsafe {
                    std::ffi::CStr::from_ptr((funcs.snd_strerror)(open_result))
                        .to_string_lossy()
                        .into_owned()
                }
            ));
        }

        let mut hw_params: *mut ffi::snd_pcm_hw_params_t = std::ptr::null_mut();
        let hw_malloc_result = unsafe { (funcs.snd_pcm_hw_params_malloc)(&mut hw_params) };
        if hw_malloc_result < 0 || hw_params.is_null() {
            unsafe { (funcs.snd_pcm_close)(pcm) };
            return Err("snd_pcm_hw_params_malloc failed".into());
        }

        let hw_any_result = unsafe { (funcs.snd_pcm_hw_params_any)(pcm, hw_params) };
        if hw_any_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params_any failed".into());
        }

        let access_result = unsafe {
            (funcs.snd_pcm_hw_params_set_access)(
                pcm,
                hw_params,
                ffi::SND_PCM_ACCESS_RW_INTERLEAVED,
            )
        };
        if access_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params_set_access failed".into());
        }

        let format_result =
            unsafe { (funcs.snd_pcm_hw_params_set_format)(pcm, hw_params, ffi::SND_PCM_FORMAT_FLOAT_LE) };
        if format_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params_set_format failed; device may not support FLOAT_LE".into());
        }

        let mut rate = sample_rate;
        let mut dir = 0;
        let rate_result = unsafe {
            (funcs.snd_pcm_hw_params_set_rate_near)(pcm, hw_params, &mut rate, &mut dir)
        };
        if rate_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params_set_rate_near failed".into());
        }

        let channels_result = unsafe {
            (funcs.snd_pcm_hw_params_set_channels)(pcm, hw_params, channels as u32)
        };
        if channels_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params_set_channels failed".into());
        }

        let hw_params_result = unsafe { (funcs.snd_pcm_hw_params)(pcm, hw_params) };
        if hw_params_result < 0 {
            unsafe {
                (funcs.snd_pcm_hw_params_free)(hw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_hw_params failed".into());
        }

        unsafe { (funcs.snd_pcm_hw_params_free)(hw_params); }

        let mut sw_params: *mut ffi::snd_pcm_sw_params_t = std::ptr::null_mut();
        let sw_malloc_result = unsafe { (funcs.snd_pcm_sw_params_malloc)(&mut sw_params) };
        if sw_malloc_result < 0 || sw_params.is_null() {
            unsafe { (funcs.snd_pcm_close)(pcm) };
            return Err("snd_pcm_sw_params_malloc failed".into());
        }

        let sw_current_result = unsafe { (funcs.snd_pcm_sw_params_current)(pcm, sw_params) };
        if sw_current_result < 0 {
            unsafe {
                (funcs.snd_pcm_sw_params_free)(sw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_sw_params_current failed".into());
        }

        unsafe {
            (funcs.snd_pcm_sw_params_set_avail_min)(pcm, sw_params, 1024);
            (funcs.snd_pcm_sw_params_set_start_threshold)(pcm, sw_params, 0);
        }

        let sw_params_result = unsafe { (funcs.snd_pcm_sw_params)(pcm, sw_params) };
        if sw_params_result < 0 {
            unsafe {
                (funcs.snd_pcm_sw_params_free)(sw_params);
                (funcs.snd_pcm_close)(pcm);
            }
            return Err("snd_pcm_sw_params failed".into());
        }

        unsafe { (funcs.snd_pcm_sw_params_free)(sw_params); }

        let prepare_result = unsafe { (funcs.snd_pcm_prepare)(pcm) };
        if prepare_result < 0 {
            unsafe { (funcs.snd_pcm_close)(pcm) };
            return Err("snd_pcm_prepare failed".into());
        }

        tracing::info!(
            "Starting ALSA stream at {} Hz, {} channels",
            sample_rate, channels
        );

        let state = Arc::new(AlsaState {
            buffer: audio_buffer,
            volume,
            playing: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            pcm: Mutex::new(SendSyncPcm(pcm)),
        });

        let state_clone = state.clone();
        let thread = thread::spawn(move || {
            AlsaAudioOutputStream::writer_loop(state_clone, funcs);
        });

        Ok(Box::new(AlsaAudioOutputStream {
            state,
            funcs,
            _lib: lib,
            thread: Mutex::new(Some(thread)),
        }))
    }
}

impl AudioOutputStream for AlsaAudioOutputStream {
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

        let pcm_ptr = match self.state.pcm.lock() {
            Ok(guard) => guard.0,
            Err(e) => e.into_inner().0,
        };

        if !pcm_ptr.is_null() {
            unsafe {
                (self.funcs.snd_pcm_close)(pcm_ptr);
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

impl Drop for AlsaAudioOutputStream {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn init() {
    // No special initialization needed for ALSA.
}
