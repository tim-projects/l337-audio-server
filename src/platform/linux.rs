use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer, ensure_runtime_dir};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

struct SendSyncStream(pipewire::stream::StreamBox<'static>);
unsafe impl Send for SendSyncStream {}
unsafe impl Sync for SendSyncStream {}

pub struct PipeWireAudioBackend;

pub struct PipeWireAudioOutputStream {
    playing: Arc<AtomicBool>,
    _stream: Option<SendSyncStream>,
}

impl AudioBackend for PipeWireAudioBackend {
    fn start_stream(
        &self,
        name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        tracing::info!("Starting PipeWire stream: name={}, sample_rate={}, channels={}", name, sample_rate, channels);
        unsafe { pipewire::init() };

        let main_loop = Box::new(pipewire::main_loop::MainLoopBox::new(None)
            .map_err(|e| format!("Failed to create PipeWire main loop: {}", e))?);

        let main_loop_leaked = Box::leak(main_loop);

        let context = Box::new(pipewire::context::ContextBox::new(&main_loop_leaked.loop_(), None)
            .map_err(|e| format!("Failed to create PipeWire context: {}", e))?);

        let context_leaked = Box::leak(context);

        let core = Box::new(context_leaked.connect(None)
            .map_err(|e| format!("Failed to connect to PipeWire: {}", e))?);

        let core_leaked = Box::leak(core);

        let playing = Arc::new(AtomicBool::new(true));

        let props = pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Audio",
            *pipewire::keys::MEDIA_CATEGORY => "Playback",
            *pipewire::keys::MEDIA_ROLE => "Music",
            *pipewire::keys::NODE_NAME => name,
            *pipewire::keys::NODE_DESCRIPTION => "L337 Audio Server",
        };

        let stream = pipewire::stream::StreamBox::new(&core_leaked, name, props)
            .map_err(|e| format!("Failed to create PipeWire stream: {}", e))?;

        let ab = audio_buffer.clone();
        let vol = volume.clone();
        let playing_cb = playing.clone();

        let listener = stream
            .add_local_listener_with_user_data((ab, vol, playing_cb))
            .state_changed(|stream, _user_data, old, new| {
                tracing::info!("PipeWire stream state changed: {:?} -> {:?}", old, new);
            })
            .process(move |stream, (ab, vol, playing_cb)| {
                tracing::debug!("PipeWire process callback called");
                let playing = playing_cb.load(Ordering::SeqCst);
                if !playing {
                    return;
                }

                let mut buf = ab.lock().unwrap();
                let available = buf.pcm.len().saturating_sub(buf.read_pos);
                let v = *vol.lock().unwrap();

                if let Some(mut buffer) = stream.dequeue_buffer() {
                    let datas = buffer.datas_mut();
                    if datas.is_empty() {
                        return;
                    }

                    let data = &mut datas[0];
                    let slice = if let Some(s) = data.data() {
                        s
                    } else {
                        return;
                    };

                    let f32_len = slice.len() / 4;
                    if f32_len == 0 {
                        return;
                    }

                    let f32_slice = unsafe {
                        std::slice::from_raw_parts_mut(slice.as_mut_ptr() as *mut f32, f32_len)
                    };

                    let to_copy = f32_len.min(available);
                    for (d, s) in f32_slice.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                        *d = s * v;
                    }
                    buf.read_pos += to_copy;

                    for sample in &mut f32_slice[to_copy..] {
                        *sample = 0.0;
                    }

                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = (channels as i32) * 4;
                    *chunk.size_mut() = (to_copy * 4) as u32;
                }
            })
            .register()
            .map_err(|e| format!("Failed to register stream listener: {}", e))?;

        let stream_flags = pipewire::stream::StreamFlags::AUTOCONNECT
            | pipewire::stream::StreamFlags::MAP_BUFFERS
            | pipewire::stream::StreamFlags::RT_PROCESS;

        let mut audio_info = pipewire::spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(pipewire::spa::param::audio::AudioFormat::F32LE);
        audio_info.set_rate(sample_rate);
        audio_info.set_channels(channels as u32);
        let mut position = [0; pipewire::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
        if channels >= 1 {
            position[0] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FL;
        }
        if channels >= 2 {
            position[1] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FR;
        }
        audio_info.set_position(position);

        let values: Vec<u8> = pipewire::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pipewire::spa::pod::Value::Object(pipewire::spa::pod::Object {
                type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Format,
                id: pipewire::spa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .map_err(|e| format!("Failed to serialize audio format: {}", e))?
        .0
        .into_inner();

        let mut params = [pipewire::spa::pod::Pod::from_bytes(&values)
            .ok_or("Failed to create Pod from bytes")?];

        stream
            .connect(
                pipewire::spa::utils::Direction::Output,
                None,
                stream_flags,
                &mut params,
            )
            .map_err(|e| format!("Failed to connect PipeWire stream: {}", e))?;
        tracing::info!("PipeWire stream connected successfully");

        stream
            .set_active(true)
            .map_err(|e| format!("Failed to activate PipeWire stream: {}", e))?;
        tracing::info!("PipeWire stream activated successfully");

        let stream = unsafe {
            std::mem::transmute::<pipewire::stream::StreamBox<'_>, pipewire::stream::StreamBox<'static>>(stream)
        };

        struct SendMainLoopPtr(*mut pipewire::sys::pw_main_loop);
        unsafe impl Send for SendMainLoopPtr {}

        impl SendMainLoopPtr {
            unsafe fn run(self) {
                let ml = &*(self.0 as *const pipewire::main_loop::MainLoop);
                ml.run();
            }
        }

        let main_loop_ptr = SendMainLoopPtr(main_loop_leaked.as_raw_ptr());

        let _leaked_listener = Box::leak(Box::new(listener));

        std::thread::spawn(move || {
            unsafe {
                main_loop_ptr.run();
            }
        });

        Ok(Box::new(PipeWireAudioOutputStream {
            playing,
            _stream: Some(SendSyncStream(stream)),
        }))
    }
}

impl AudioOutputStream for PipeWireAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.playing.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.playing.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&mut self) {
        self.playing.store(false, Ordering::SeqCst);
        self._stream = None;
    }
}

pub fn init() {
    ensure_runtime_dir();
}

#[cfg(test)]
mod tests {
    use crate::platform::common::runtime_dir;

    #[test]
    fn test_runtime_dir_is_absolute() {
        let dir = runtime_dir();
        assert!(dir.is_absolute());
    }
}
