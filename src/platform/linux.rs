use crate::platform::common::{AudioBackend, AudioBuffer, AudioOutputStream, ensure_runtime_dir};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub enum Command {
    SetActive(bool),
    Disconnect,
}

#[derive(Default)]
pub struct PipeWireAudioBackend;

pub struct PipeWireAudioOutputStream {
    playing: Arc<AtomicBool>,
    backend_error: Arc<AtomicBool>,
    cmd: pipewire::channel::Sender<Command>,
}

fn terminal_pw_state(new: &pipewire::stream::StreamState) -> bool {
    matches!(
        new,
        pipewire::stream::StreamState::Error(_)
    )
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
        tracing::info!(
            "Starting PipeWire stream: name={}, sample_rate={}, channels={}",
            name,
            sample_rate,
            channels
        );
        pipewire::init();

        let playing = Arc::new(AtomicBool::new(false));
        let backend_error = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_rx) = pipewire::channel::channel::<Command>();
        let cmd_tx_out = cmd_tx.clone();

        let name_owned = name.to_string();
        let ab = audio_buffer.clone();
        let vol = volume.clone();
        let playing_cb = playing.clone();
        let backend_error_cb = backend_error.clone();
        let channels_captured = channels;

        std::thread::spawn(move || {
            let main_loop = match pipewire::main_loop::MainLoopBox::new(None) {
                Ok(ml) => ml,
                Err(e) => {
                    tracing::error!("Failed to create PipeWire main loop: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };
            let main_loop: &'static pipewire::main_loop::MainLoopBox =
                Box::leak(Box::new(main_loop));

            let context = match pipewire::context::ContextBox::new(&main_loop.loop_(), None) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to create PipeWire context: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };
            let context: &'static pipewire::context::ContextBox = Box::leak(Box::new(context));

            let core = match context.connect(None) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to connect to PipeWire: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };
            let core: &'static pipewire::core::CoreBox = Box::leak(Box::new(core));

            let props = pipewire::properties::properties! {
                *pipewire::keys::MEDIA_TYPE => "Audio",
                *pipewire::keys::MEDIA_CATEGORY => "Playback",
                *pipewire::keys::MEDIA_ROLE => "Music",
                *pipewire::keys::NODE_NAME => name_owned.as_str(),
                *pipewire::keys::NODE_DESCRIPTION => "L337 Audio Server",
            };

            let stream = match pipewire::stream::StreamBox::new(core, &name_owned, props) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to create PipeWire stream: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };

            let ab_cb = ab.clone();
            let vol_cb = vol.clone();
            let playing_cb2 = playing_cb.clone();
            let be_cb2 = backend_error_cb.clone();

            let _listener = match stream
                .add_local_listener_with_user_data((ab_cb, vol_cb, playing_cb2, channels_captured))
                .state_changed(|_stream, user_data, old, new| {
                    if terminal_pw_state(&new) {
                        tracing::warn!(
                            "PipeWire stream entered error state: {:?} -> {:?}",
                            old,
                            new
                        );
                        let (ab, _vol, playing_cb, _channels) = user_data;
                        playing_cb.store(false, Ordering::SeqCst);
                        if let Ok(mut buf) = ab.lock() {
                            buf.backend_error.store(true, Ordering::SeqCst);
                            buf.pcm.clear();
                            buf.read_pos = 0;
                            buf.consumed_total = 0;
                        }
                    } else if matches!(new, pipewire::stream::StreamState::Paused | pipewire::stream::StreamState::Streaming) {
                        tracing::debug!("PipeWire stream state: {:?} -> {:?}", old, new);
                    }
                })
                .process(move |stream, (ab, vol, playing_cb, channels)| {
                    let playing = playing_cb.load(Ordering::SeqCst);
                    if !playing {
                        return;
                    }

                    let mut buf = ab.lock().unwrap_or_else(|e| e.into_inner());

                    if buf.backend_error.load(Ordering::SeqCst) {
                        return;
                    }

                    if buf.enters_over_window() {
                        tracing::debug!("AudioBuffer over rebuffer window; draining (paced)");
                    }

                    let available = buf.pcm.len().saturating_sub(buf.read_pos);

                    if available > 0 {
                        let v = *vol.lock().unwrap_or_else(|e| e.into_inner());
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
                            for (d, s) in f32_slice.iter_mut().take(to_copy).zip(buf.pcm[buf.read_pos..].iter()) {
                                *d = s * v;
                            }
                            buf.read_pos += to_copy;

                            if buf.read_pos >= (1 << 20) {
                                buf.reclaim_consumed();
                            }

                            for sample in &mut f32_slice[to_copy..] {
                                *sample = 0.0;
                            }

                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            *chunk.stride_mut() = (*channels as i32) * 4;
                            *chunk.size_mut() = (to_copy * 4) as u32;
                        }
                    } else {
                        drop(buf);
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

                            for sample in f32_slice.iter_mut() {
                                *sample = 0.0;
                            }

                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            *chunk.stride_mut() = (*channels as i32) * 4;
                            *chunk.size_mut() = 0;
                        }
                    }
                })
                .register()
            {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to register stream listener: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };

            let stream_flags = pipewire::stream::StreamFlags::AUTOCONNECT
                | pipewire::stream::StreamFlags::MAP_BUFFERS
                | pipewire::stream::StreamFlags::RT_PROCESS;

            let mut audio_info = pipewire::spa::param::audio::AudioInfoRaw::new();
            audio_info.set_format(pipewire::spa::param::audio::AudioFormat::F32LE);
            audio_info.set_rate(sample_rate);
            audio_info.set_channels(channels as u32);
            let mut position = [0u32; pipewire::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
            if channels >= 1 {
                position[0] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FL;
            }
            if channels >= 2 {
                position[1] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FR;
            }
            audio_info.set_position(position);

            let values: Vec<u8> = match pipewire::spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &pipewire::spa::pod::Value::Object(pipewire::spa::pod::Object {
                    type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Format,
                    id: pipewire::spa::sys::SPA_PARAM_EnumFormat,
                    properties: audio_info.into(),
                }),
            ) {
                Ok(v) => v.0.into_inner(),
                Err(e) => {
                    tracing::error!("Failed to serialize audio format: {}", e);
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };

            let mut params = match pipewire::spa::pod::Pod::from_bytes(&values) {
                Some(p) => [p],
                None => {
                    tracing::error!("Failed to create Pod from bytes");
                    backend_error_cb.store(true, Ordering::SeqCst);
                    return;
                }
            };

            if let Err(e) = stream.connect(
                pipewire::spa::utils::Direction::Output,
                None,
                stream_flags,
                &mut params,
            ) {
                tracing::error!("Failed to connect PipeWire stream: {}", e);
                backend_error_cb.store(true, Ordering::SeqCst);
                return;
            }
            tracing::info!("PipeWire stream connected successfully");

            if let Err(e) = stream.set_active(true) {
                tracing::error!("Failed to activate PipeWire stream: {}", e);
                backend_error_cb.store(true, Ordering::SeqCst);
                return;
            }
            tracing::info!("PipeWire stream activated successfully");

            let loop_ref = main_loop.loop_();
            let _receiver = cmd_rx.attach(loop_ref, move |cmd| match cmd {
                    Command::SetActive(active) => {
                        let _ = stream.set_active(active);
                    }
                    Command::Disconnect => {
                        let _ = stream.disconnect();
                        main_loop.quit();
                    }
                });

            main_loop.run();
            // stream, listener, context, core, main_loop dropped here on main loop thread
        });

        Ok(Box::new(PipeWireAudioOutputStream {
            playing,
            backend_error,
            cmd: cmd_tx_out,
        }))
    }
}

impl AudioOutputStream for PipeWireAudioOutputStream {
    fn play(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(true, Ordering::SeqCst);
        let _ = self.cmd.send(Command::SetActive(true));
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        let _ = self.cmd.send(Command::SetActive(false));
        Ok(())
    }

    fn stop(&mut self) {
        self.backend_error.store(false, Ordering::SeqCst);
        self.playing.store(false, Ordering::SeqCst);
        let _ = self.cmd.send(Command::Disconnect);
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
