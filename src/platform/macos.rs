use crate::platform::common::{runtime_dir, ensure_runtime_dir, AudioBackend, AudioOutputStream, AudioBuffer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tracing::warn;
use coreaudio::audio_unit::{AudioUnit, IOType, Scope, StreamFormat, Type};
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{Args, Data, Raw};

pub struct CoreAudioAudioBackend;

pub struct CoreAudioAudioOutputStream {
    playing: Arc<AtomicBool>,
    backend_error: Arc<AtomicBool>,
    _audio_unit: Mutex<AudioUnit>,
}

impl AudioBackend for CoreAudioAudioBackend {
    fn start_stream(
        &self,
        _name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        let mut audio_unit = coreaudio::audio_unit::AudioUnit::new(
            Type::IO(IOType::DefaultOutput),
        )
        .map_err(|e| format!("Failed to create AudioUnit: {}", e))?;

        let ab = audio_buffer.clone();
        let vol = volume.clone();
        let playing = Arc::new(AtomicBool::new(false));
        let playing_cb = playing.clone();
        let backend_error = Arc::new(AtomicBool::new(false));
        let backend_error_cb = backend_error.clone();

        audio_unit
            .set_render_callback(move |args: Args<Raw>| {
                if backend_error_cb.load(Ordering::SeqCst) {
                    let buf_list = unsafe { &*args.data };
                    for i in 0..buf_list.mNumberBuffers {
                        let buffer = &buf_list.mBuffers[i as usize];
                        if !buffer.mData.is_null() {
                            let len = buffer.mDataByteSize as usize / std::mem::size_of::<f32>();
                            let slice = unsafe {
                                std::slice::from_raw_parts_mut(buffer.mData as *mut f32, len)
                            };
                            for sample in slice.iter_mut() {
                                *sample = 0.0;
                            }
                        }
                    }
                    return Ok(());
                }

                let playing = playing_cb.load(Ordering::SeqCst);
                if !playing {
                    let buf_list = unsafe { &*args.data };
                    for i in 0..buf_list.mNumberBuffers {
                        let buffer = &buf_list.mBuffers[i as usize];
                        if !buffer.mData.is_null() {
                            let len = buffer.mDataByteSize as usize / std::mem::size_of::<f32>();
                            let slice = unsafe {
                                std::slice::from_raw_parts_mut(buffer.mData as *mut f32, len)
                            };
                            for sample in slice.iter_mut() {
                                *sample = 0.0;
                            }
                        }
                    }
                    return Ok(());
                }

                let mut buf = ab.lock().unwrap();
                if buf.pcm.len() > buf.max_bytes / 4 {
                    buf.pcm.clear();
                    buf.read_pos = 0;
                    buf.backend_error.store(true, Ordering::SeqCst);
                    drop(buf);
                    tracing::warn!("CoreAudio buffer cap exceeded, signalling backend error");
                    return Err(());
                }

                let available = buf.pcm.len().saturating_sub(buf.read_pos);
                let vol = *vol.lock().unwrap();

                let buf_list = unsafe { &*args.data };
                let num_channels = buf_list.mNumberBuffers as usize;
                if num_channels == 0 {
                    return Ok(());
                }
                let frames_to_copy = available / num_channels;
                let buffer_frames = if buf_list.mBuffers[0].mDataByteSize > 0 {
                    (buf_list.mBuffers[0].mDataByteSize as usize / std::mem::size_of::<f32>()) / num_channels
                } else {
                    0
                };
                let frames = frames_to_copy.min(buffer_frames);

                for ch in 0..num_channels {
                    let buffer = &buf_list.mBuffers[ch];
                    if buffer.mData.is_null() {
                        continue;
                    }
                    let len = buffer.mDataByteSize as usize / std::mem::size_of::<f32>();
                    let slice = unsafe {
                        std::slice::from_raw_parts_mut(buffer.mData as *mut f32, len)
                    };
                    for frame in 0..frames {
                        let src_idx = buf.read_pos + frame * num_channels + ch;
                        if src_idx < buf.pcm.len() {
                            slice[frame] = buf.pcm[src_idx] * vol;
                        } else {
                            slice[frame] = 0.0;
                        }
                    }
                    for frame in frames..len {
                        slice[frame] = 0.0;
                    }
                }
                buf.read_pos += frames * num_channels;

                Ok(())
            })
            .map_err(|e| format!("Failed to set render callback: {}", e))?;

        let asbd = StreamFormat {
            sample_rate: sample_rate as f64,
            sample_format: coreaudio::audio_unit::SampleFormat::F32,
            flags: LinearPcmFlags::empty(),
            channels_per_frame: channels as u32,
        }
        .to_asbd();

        let stream_format = StreamFormat::from_asbd(asbd)
            .map_err(|e| format!("Failed to create stream format: {}", e))?;

        audio_unit
            .set_stream_format(stream_format, Scope::Output)
            .map_err(|e| format!("Failed to set stream format: {}", e))?;

        audio_unit
            .start()
            .map_err(|e| format!("Failed to start AudioUnit: {}", e))?;

        Ok(Box::new(CoreAudioAudioOutputStream {
            playing,
            backend_error,
            _audio_unit: Mutex::new(audio_unit),
        }))
    }
}

impl AudioOutputStream for CoreAudioAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
    }
}

pub fn init() {
    ensure_runtime_dir();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        assert!(runtime_dir().is_absolute());
    }
}
