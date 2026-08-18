use crate::platform::common::{runtime_dir, AudioBackend, AudioOutputStream, AudioBuffer};
use std::sync::Arc;
use std::sync::Mutex;

pub struct CoreAudioAudioBackend;

pub struct CoreAudioAudioOutputStream {
    _audio_unit: coreaudio::audio_unit::AudioUnit,
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
            coreaudio::audio_unit::AudioUnitType::Output,
        )
        .map_err(|e| format!("Failed to create AudioUnit: {}", e))?;

        let ab = audio_buffer.clone();
        let vol = volume.clone();

        audio_unit
            .set_render_callback(move |data: &mut [f32], _| {
                let mut buf = ab.lock().unwrap();
                let available = buf.pcm.len().saturating_sub(buf.read_pos);
                let vol = *vol.lock().unwrap();

                let to_copy = data.len().min(available);
                for (d, s) in data.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                    *d = s * vol;
                }
                buf.read_pos += to_copy;

                for sample in &mut data[to_copy..] {
                    *sample = 0.0;
                }

                Ok(())
            })
            .map_err(|e| format!("Failed to set render callback: {}", e))?;

        let stream_format = coreaudio::audio_unit::StreamFormat::new()
            .with_sample_rate(sample_rate as f64)
            .with_channels(channels as usize);

        audio_unit
            .set_stream_format(&stream_format)
            .map_err(|e| format!("Failed to set stream format: {}", e))?;

        audio_unit
            .start()
            .map_err(|e| format!("Failed to start AudioUnit: {}", e))?;

        Ok(Box::new(CoreAudioAudioOutputStream {
            _audio_unit: audio_unit,
        }))
    }
}

impl AudioOutputStream for CoreAudioAudioOutputStream {
    fn play(&mut self) -> Result<(), String> { Ok(()) }
    fn pause(&mut self) -> Result<(), String> { Ok(()) }
    fn stop(&mut self) {}
}

pub fn init() {
    let _dir = runtime_dir();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        assert!(runtime_dir().is_absolute());
    }
}
