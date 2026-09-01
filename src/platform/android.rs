use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};

#[allow(non_camel_case_types)]
mod ffi {
    use std::os::raw::{c_int, c_void};

    pub type AAudioStream = c_void;
    pub type AAudioStreamBuilder = c_void;

    #[repr(C)]
    pub enum AAudioStreamDirection {
        Input = 0,
        Output = 1,
    }

    #[repr(C)]
    pub enum AAudioFormat {
        Invalid = -1,
        Unspecified = 0,
        I16 = 1,
        Float = 2,
        I24 = 3,
        I32 = 4,
    }

    #[repr(C)]
    pub enum AAudioSharingMode {
        Exclusive = 0,
        Shared = 1,
    }

    #[repr(C)]
    pub enum AAudioPerformanceMode {
        None = 0,
        PowerSaving = 1,
        LowLatency = 2,
    }

    pub const AAUDIO_DIRECTION_OUTPUT: AAudioStreamDirection = AAudioStreamDirection::Output;
    pub const AAUDIO_FORMAT_FLOAT: AAudioFormat = AAudioFormat::Float;
    pub const AAUDIO_SHARING_MODE_SHARED: AAudioSharingMode = AAudioSharingMode::Shared;
    pub const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: AAudioPerformanceMode = AAudioPerformanceMode::LowLatency;
    pub const AAUDIO_CALLBACK_RESULT_CONTINUE: c_int = 0;
    pub const AAUDIO_OK: c_int = 0;

    extern "C" {
        pub fn AAudio_createStreamBuilder() -> *mut AAudioStreamBuilder;
        pub fn AAudioStreamBuilder_setDirection(
            builder: *mut AAudioStreamBuilder,
            direction: AAudioStreamDirection,
        );
        pub fn AAudioStreamBuilder_setFormat(builder: *mut AAudioStreamBuilder, format: AAudioFormat);
        pub fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, sampleRate: i32);
        pub fn AAudioStreamBuilder_setChannelCount(builder: *mut AAudioStreamBuilder, channelCount: i32);
        pub fn AAudioStreamBuilder_setSharingMode(
            builder: *mut AAudioStreamBuilder,
            sharingMode: AAudioSharingMode,
        );
        pub fn AAudioStreamBuilder_setPerformanceMode(
            builder: *mut AAudioStreamBuilder,
            performanceMode: AAudioPerformanceMode,
        );
        pub fn AAudioStreamBuilder_setDataCallback(
            builder: *mut AAudioStreamBuilder,
            callback: extern "C" fn(
                stream: *mut AAudioStream,
                userData: *mut c_void,
                audioData: *mut c_void,
                numFrames: i32,
            ) -> c_int,
        );
        pub fn AAudioStreamBuilder_setUserData(
            builder: *mut AAudioStreamBuilder,
            userData: *mut c_void,
        );
        pub fn AAudioStreamBuilder_setBufferCapacityInFrames(
            builder: *mut AAudioStreamBuilder,
            capacityFrames: i32,
        );
        pub fn AAudioStreamBuilder_openStream(
            builder: *mut AAudioStreamBuilder,
            stream: *mut *mut AAudioStream,
        ) -> c_int;
        pub fn AAudioStream_requestStart(stream: *mut AAudioStream) -> c_int;
        pub fn AAudioStream_requestPause(stream: *mut AAudioStream) -> c_int;
        pub fn AAudioStream_requestStop(stream: *mut AAudioStream) -> c_int;
        pub fn AAudioStream_close(stream: *mut AAudioStream) -> c_int;
        pub fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder);
        pub fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    }

    #[link(name = "aaudio")]
    extern "C" {}
}

struct AndroidState {
    buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    playing: Arc<AtomicBool>,
}

pub struct AndroidAudioBackend;

pub struct AndroidAudioOutputStream {
    stream: *mut ffi::AAudioStream,
    builder: *mut ffi::AAudioStreamBuilder,
    state: *mut AndroidState,
}

unsafe impl Send for AndroidAudioOutputStream {}
unsafe impl Sync for AndroidAudioOutputStream {}

extern "C" fn data_callback(
    _stream: *mut ffi::AAudioStream,
    userdata: *mut std::os::raw::c_void,
    audio_data: *mut std::os::raw::c_void,
    num_frames: i32,
) -> i32 {
    let state = unsafe { &*(userdata as *const AndroidState) };

    let channels = {
        let buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.channels
    };
    let channels = channels as usize;
    if channels == 0 {
        return ffi::AAUDIO_CALLBACK_RESULT_CONTINUE;
    }

    let frames = num_frames as usize;
    let total_samples = frames * channels;
    let dst = audio_data as *mut f32;
    let dst = unsafe { std::slice::from_raw_parts_mut(dst, total_samples) };

    let playing = state.playing.load(Ordering::Relaxed);
    let volume = *state.volume.lock().unwrap_or_else(|e| e.into_inner());

    if !playing {
        for sample in dst.iter_mut() {
            *sample = 0.0;
        }
        return ffi::AAUDIO_CALLBACK_RESULT_CONTINUE;
    }

    let mut buf = state.buffer.lock().unwrap_or_else(|e| e.into_inner());
    let available = buf.pcm.len().saturating_sub(buf.read_pos);
    let to_copy = available.min(total_samples);

    if to_copy > 0 {
        let src = &buf.pcm[buf.read_pos..buf.read_pos + to_copy];
        for (dst_sample, src_sample) in dst.iter_mut().zip(src.iter()) {
            *dst_sample = src_sample * volume;
        }
        buf.read_pos += to_copy;
    }

    if to_copy < total_samples {
        for sample in &mut dst[to_copy..] {
            *sample = 0.0;
        }
    }

    ffi::AAUDIO_CALLBACK_RESULT_CONTINUE
}

impl AudioBackend for AndroidAudioBackend {
    fn start_stream(
        &self,
        _name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        let state = Box::new(AndroidState {
            buffer: audio_buffer,
            volume,
            playing: Arc::new(AtomicBool::new(false)),
        });
        let state_ptr = Box::into_raw(state);
        let userdata = state_ptr as *mut std::os::raw::c_void;

        let builder = unsafe { ffi::AAudio_createStreamBuilder() };
        if builder.is_null() {
            unsafe { drop(Box::from_raw(state_ptr)) };
            return Err("AAudio_createStreamBuilder returned null".into());
        }

        unsafe {
            ffi::AAudioStreamBuilder_setDirection(builder, ffi::AAUDIO_DIRECTION_OUTPUT);
            ffi::AAudioStreamBuilder_setFormat(builder, ffi::AAUDIO_FORMAT_FLOAT);
            ffi::AAudioStreamBuilder_setSampleRate(builder, sample_rate as i32);
            ffi::AAudioStreamBuilder_setChannelCount(builder, channels as i32);
            ffi::AAudioStreamBuilder_setSharingMode(builder, ffi::AAUDIO_SHARING_MODE_SHARED);
            ffi::AAudioStreamBuilder_setPerformanceMode(
                builder,
                ffi::AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            );
            ffi::AAudioStreamBuilder_setDataCallback(builder, data_callback);
            ffi::AAudioStreamBuilder_setUserData(builder, userdata);
            ffi::AAudioStreamBuilder_setBufferCapacityInFrames(builder, 65536);
        }

        let mut stream: *mut ffi::AAudioStream = std::ptr::null_mut();
        let result = unsafe {
            ffi::AAudioStreamBuilder_openStream(builder, &mut stream as *mut *mut ffi::AAudioStream)
        };

        if result != ffi::AAUDIO_OK || stream.is_null() {
            unsafe {
                ffi::AAudioStreamBuilder_delete(builder);
                drop(Box::from_raw(state_ptr));
            }
            return Err(format!(
                "AAudioStreamBuilder_openStream failed with result {}",
                result
            ));
        }

        let actual_rate = unsafe { ffi::AAudioStream_getSampleRate(stream) };
        if actual_rate != sample_rate as i32 {
            tracing::warn!(
                "AAudio stream sample rate mismatch: requested {}, got {}",
                sample_rate,
                actual_rate
            );
        }

        let out = AndroidAudioOutputStream {
            stream,
            builder,
            state: state_ptr,
        };

        tracing::info!("Starting AAudio stream at {} Hz, {} channels", sample_rate, channels);
        Ok(Box::new(out))
    }
}

impl AudioOutputStream for AndroidAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        let state = unsafe { &*self.state };
        state.playing.store(true, Ordering::Relaxed);

        let result = unsafe { ffi::AAudioStream_requestStart(self.stream) };
        if result != ffi::AAUDIO_OK {
            return Err(format!("AAudioStream_requestStart failed with {}", result));
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        let state = unsafe { &*self.state };
        state.playing.store(false, Ordering::Relaxed);

        let result = unsafe { ffi::AAudioStream_requestPause(self.stream) };
        if result != ffi::AAUDIO_OK {
            return Err(format!("AAudioStream_requestPause failed with {}", result));
        }
        Ok(())
    }

    fn stop(&mut self) {
        unsafe {
            let _ = ffi::AAudioStream_requestStop(self.stream);
            let _ = ffi::AAudioStream_close(self.stream);
            ffi::AAudioStreamBuilder_delete(self.builder);
        }
    }
}

impl Drop for AndroidAudioOutputStream {
    fn drop(&mut self) {
        if !self.stream.is_null() {
            unsafe {
                let _ = ffi::AAudioStream_requestStop(self.stream);
                let _ = ffi::AAudioStream_close(self.stream);
                ffi::AAudioStreamBuilder_delete(self.builder);
            }
            self.stream = std::ptr::null_mut();
            self.builder = std::ptr::null_mut();
        }
        if !self.state.is_null() {
            unsafe {
                drop(Box::from_raw(self.state));
            }
            self.state = std::ptr::null_mut();
        }
    }
}
