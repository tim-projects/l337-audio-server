use crate::platform::common::{runtime_dir, AudioBackend, AudioOutputStream, AudioBuffer};
use std::sync::Arc;
use std::sync::Mutex;

pub struct WasapiAudioBackend;

pub struct WasapiAudioOutputStream {
    _audio_client: wasapi::AudioClient,
    _render_client: wasapi::RenderClient,
}

impl AudioBackend for WasapiAudioBackend {
    fn start_stream(
        &self,
        _name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        unsafe { ole32::CoInitializeEx(None, ole32::COINIT_MULTITHREADED) };

        let collection = wasapi::DeviceCollection::new(&wasapi::DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("Failed to enumerate devices: {}", e))?;

        let device = collection
            .get_default(wasapi::DEVICE_ROLE_CONSOLE, wasapi::DATAFLOW_RENDER)
            .map_err(|e| format!("Failed to get default render device: {}", e))?;

        let audio_client = device
            .get_iaudioclient()
            .map_err(|e| format!("Failed to get AudioClient: {}", e))?;

        let mix_format = audio_client
            .get_mix_format()
            .map_err(|e| format!("Failed to get mix format: {}", e))?;

        let buffer_frames = (sample_rate as u32 / 10) as u32;

        audio_client
            .initialize(
                wasapi::AUDCLNT_SHAREMODE_SHARED,
                wasapi::AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                buffer_frames * mix_format.get_block_align() as u32,
                0,
                &mix_format,
                None,
            )
            .map_err(|e| format!("Failed to initialize AudioClient: {}", e))?;

        let render_client = audio_client
            .get_service()
            .map_err(|e| format!("Failed to get RenderClient: {}", e))?;

        let event = unsafe {
            winapi::um::synchapi::CreateEventW(
                std::ptr::null_mut(),
                0,
                0,
                std::ptr::null(),
            )
        };

        if event.is_null() {
            return Err("Failed to create event handle".to_string());
        }

        audio_client
            .set_event_handle(event)
            .map_err(|e| format!("Failed to set event handle: {}", e))?;

        let ab = audio_buffer.clone();
        let vol = volume.clone();

        std::thread::spawn(move || {
            loop {
                unsafe {
                    winapi::um::synchapi::WaitForSingleObject(
                        event,
                        winapi::um::winbase::INFINITE,
                    )
                };

                let mut buffer = match render_client.get_buffer(buffer_frames as u32) {
                    Ok(b) => b,
                    Err(_) => break,
                };

                let data = buffer.data_mut();
                if data.is_empty() {
                    let _ = render_client.release_buffer(buffer_frames as u32, 0);
                    break;
                }

                let mut buf = ab.lock().unwrap();
                let available = buf.pcm.len().saturating_sub(buf.read_pos);
                let v = *vol.lock().unwrap();

                let to_copy = data.len().min(available);
                for (d, s) in data.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                    *d = s * v;
                }
                buf.read_pos += to_copy;

                for sample in &mut data[to_copy..] {
                    *sample = 0.0;
                }

                let _ = render_client.release_buffer(buffer_frames as u32, 0);
            }
        });

        audio_client
            .start()
            .map_err(|e| format!("Failed to start AudioClient: {}", e))?;

        Ok(Box::new(WasapiAudioOutputStream {
            _audio_client: audio_client,
            _render_client: render_client,
        }))
    }
}

impl AudioOutputStream for WasapiAudioOutputStream {
    fn play(&mut self) -> Result<(), String> { Ok(()) }
    fn pause(&mut self) -> Result<(), String> { Ok(()) }
    fn stop(&mut self) {}
}

pub fn init() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        assert!(runtime_dir().is_absolute());
    }
}
