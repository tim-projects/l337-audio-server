use crate::api::models::{PlayerStateLabel, PlayerStatus, Track};
use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};
use crate::player::storage::StorageManager;
use rubato::Resampler;
use rubato::SincFixedIn;
use rubato::SincInterpolationParameters;
use rubato::SincInterpolationType;
use rubato::WindowFunction;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tracing::{error, info};

struct StreamingPlayback {
    download_handle: Option<tokio::task::JoinHandle<()>>,
    decode_handle: tokio::task::JoinHandle<()>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YouTubeBlockType {
    RateLimit,
    Captcha,
    BotDetected,
    IpBlocked,
}

impl YouTubeBlockType {
    pub fn as_str(&self) -> &'static str {
        match self {
            YouTubeBlockType::RateLimit => "rate_limit",
            YouTubeBlockType::Captcha => "captcha",
            YouTubeBlockType::BotDetected => "bot_detected",
            YouTubeBlockType::IpBlocked => "ip_blocked",
        }
    }
}

#[derive(Debug, Clone)]
pub struct YouTubeError {
    pub block_type: YouTubeBlockType,
    pub message: String,
}

impl std::fmt::Display for YouTubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for YouTubeError {}

#[derive(Debug)]
pub enum EngineError {
    YouTube(YouTubeError),
    Other(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::YouTube(e) => write!(f, "{}", e),
            EngineError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for EngineError {}

pub struct PlayerEngine {
    stream: Option<Box<dyn AudioOutputStream>>,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    pub storage: StorageManager,
    pub current_track: Option<Track>,
    pub state: PlayerStateLabel,
    pub speed: f32,
    pub volume_val: f32,
    pub duration_sec: Option<u64>,
    pub position_sec: u64,
    file_sample_rate: u32,
    channels: u16,
    streaming: Option<StreamingPlayback>,
}

impl PlayerEngine {
    pub fn new(storage: StorageManager) -> Result<Self, String> {
        let audio_buffer = Arc::new(Mutex::new(AudioBuffer::new(48000, 0)));
        let volume = Arc::new(Mutex::new(1.0));

        let backend: Box<dyn AudioBackend> = {
            #[cfg(all(target_os = "linux", feature = "backend"))]
            {
                Box::new(crate::platform::linux::PipeWireAudioBackend)
            }
            #[cfg(all(target_os = "linux", feature = "alsa", not(feature = "backend")))]
            {
                Box::new(crate::platform::alsa::AlsaAudioBackend)
            }
            #[cfg(all(target_os = "macos", feature = "backend"))]
            {
                Box::new(crate::platform::macos::CoreAudioAudioBackend)
            }
            #[cfg(all(target_os = "windows", feature = "backend"))]
            {
                Box::new(crate::platform::windows::WasapiAudioBackend)
            }
            #[cfg(all(target_os = "android", feature = "backend"))]
            {
                Box::new(crate::platform::android::AndroidAudioBackend)
            }
            #[cfg(not(any(feature = "backend", feature = "alsa")))]
            {
                Box::new(crate::platform::common::NoopAudioBackend)
            }
            #[cfg(all(
                not(any(
                    all(target_os = "linux", feature = "backend"),
                    all(target_os = "macos", feature = "backend"),
                    all(target_os = "windows", feature = "backend"),
                    all(target_os = "android", feature = "backend")
                )),
                feature = "backend"
            ))]
            {
                Box::new(crate::platform::common::NoopAudioBackend)
            }
        };

        let stream = backend.start_stream(
            "l337-audio-server",
            48000,
            2,
            audio_buffer.clone(),
            volume.clone(),
        )?;

        Ok(Self {
            stream: Some(stream),
            audio_buffer,
            volume: volume.clone(),
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            volume_val: 1.0,
            duration_sec: None,
            position_sec: 0,
            file_sample_rate: 0,
            channels: 0,
            streaming: None,
        })
    }

    pub fn new_dummy(storage: StorageManager) -> Self {
        Self {
            stream: Some(Box::new(crate::platform::common::NoopAudioOutputStream)),
            audio_buffer: Arc::new(Mutex::new(AudioBuffer::new(48000, 0))),
            volume: Arc::new(Mutex::new(1.0)),
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            volume_val: 1.0,
            duration_sec: None,
            position_sec: 0,
            file_sample_rate: 0,
            channels: 0,
            streaming: None,
        }
    }

pub async fn play_track(&mut self, track: Track) -> Result<(), EngineError> {
        self.stop();
        self.current_track = Some(track.clone());

        let cached_path = self.storage.get_path_for_track(&track.track_id);
        if cached_path.exists() {
            info!("play_track: cache hit for {}", track.track_id);
            let slot = self.storage.get_active_slot_path("current");
            let _ = fs::copy(&cached_path, &slot).await;
            self.load_and_play("current").await;
            self.storage.update_access(&track.track_id, 0).await;
            return Ok(());
        }

        if is_youtube_url(&track.stream_url) {
            info!("play_track: YouTube streaming for {}", track.stream_url);
            if let Err(e) = self.start_streaming_playback(&track.stream_url).await {
                if let Some(yt_err) = e.downcast_ref::<YouTubeError>() {
                    return Err(EngineError::YouTube(yt_err.clone()));
                }
                error!("YouTube streaming failed, falling back to download: {}", e);
                let path = self.storage.get_active_slot_path("current");
                if let Err(e) = download_stream(&track.stream_url, &path).await {
                    let err_box = e;
                    if let Some(yt_err) = err_box.downcast_ref::<YouTubeError>() {
                        return Err(EngineError::YouTube(yt_err.clone()));
                    }
                    return Err(EngineError::Other(format!("Failed to download track: {}", err_box)));
                }
                self.load_and_play("current").await;
                self.persist_slot(&track.track_id, &path).await;
            }
            return Ok(());
        }

        let path = self.storage.get_active_slot_path("current");
        info!(
            "play_track: downloading {} -> {}",
            track.stream_url,
            path.display()
        );

        if let Err(e) = download_stream(&track.stream_url, &path).await {
            let err_box = e;
            if let Some(yt_err) = err_box.downcast_ref::<YouTubeError>() {
                return Err(EngineError::YouTube(yt_err.clone()));
            }
            error!("play_track: download failed for {}: {}", track.stream_url, err_box);
            return Err(EngineError::Other(format!("Failed to download track: {}", err_box)));
        }

        match tokio::fs::metadata(&path).await {
            Ok(meta) => info!(
                "play_track: downloaded {} bytes to {}",
                meta.len(),
                path.display()
            ),
            Err(e) => {
                error!("play_track: slot file missing after download: {}", e);
                return Err(EngineError::Other(format!("Slot file missing after download: {}", e)));
            }
        }

        self.load_and_play("current").await;
        self.persist_slot(&track.track_id, &path).await;
        Ok(())
    }

    pub async fn play_pushed(
        &mut self,
        track_id: &str,
        slot: &str,
        title: Option<String>,
        artist: Option<String>,
    ) {
        self.stop();
        self.current_track = Some(Track {
            track_id: track_id.to_string(),
            stream_url: String::new(),
            title,
            artist,
            duration: None,
        });

        let path = self.storage.get_active_slot_path(slot);
        self.load_and_play(slot).await;
        self.persist_slot(track_id, &path).await;
    }

    async fn persist_slot(&self, track_id: &str, slot_path: &PathBuf) {
        let dest_path = self.storage.get_path_for_track(track_id);
        let _ = fs::copy(slot_path, dest_path).await;
        if let Ok(meta) = fs::metadata(slot_path).await {
            self.storage.update_access(track_id, meta.len()).await;
        }
    }

    pub async fn load_and_play(&mut self, slot: &str) {
        let path = self.storage.get_active_slot_path(slot);

        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                error!(
                    "load_and_play({}): cannot read {}: {}",
                    slot,
                    path.display(),
                    e
                );
                return;
            }
        };

        let decode = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || decode_to_pcm(bytes)))
        })
        .await;

        let (pcm, file_sample_rate, channels) = match decode {
            Ok(Ok(Ok(result))) => result,
            Ok(Ok(Err(e))) => {
                error!(
                    "load_and_play({}): decode failed on {}: {:?}",
                    slot,
                    path.display(),
                    e
                );
                return;
            }
            Ok(Err(_)) => {
                error!(
                    "load_and_play({}): decoder panicked on {} (unsupported/corrupt container)",
                    slot,
                    path.display()
                );
                return;
            }
            Err(e) => {
                error!("load_and_play({}): decode task join error: {}", slot, e);
                return;
            }
        };

        self.file_sample_rate = file_sample_rate;
        self.channels = channels;
        self.duration_sec = Some(((pcm.len() / channels as usize) as f64 / file_sample_rate as f64).round() as u64);

        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.sample_rate
        };

        let playback_pcm =
            resample_interleaved(&pcm, channels, file_sample_rate, device_rate, self.speed);

        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm = playback_pcm;
        buf.read_pos = 0;
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        drop(buf);

        self.position_sec = 0;
        self.state = PlayerStateLabel::Playing;
        info!(
            "load_and_play({}): Playing (duration {:?}s)",
            slot, self.duration_sec
        );
    }

    pub fn pause(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = stream.pause() {
                error!("pause failed: {}", e);
            }
        }
        self.state = PlayerStateLabel::Paused;
    }

    pub fn resume(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = stream.play() {
                error!("resume failed: {}", e);
            }
        }
        self.state = PlayerStateLabel::Playing;
    }

    pub fn stop(&mut self) {
        if let Some(streaming) = self.streaming.take() {
            streaming.cancel.store(true, Ordering::SeqCst);
            if let Some(handle) = streaming.download_handle {
                handle.abort();
            }
            streaming.decode_handle.abort();
        }
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm.clear();
        buf.read_pos = 0;
        drop(buf);
        self.position_sec = 0;
        self.state = PlayerStateLabel::Stopped;
    }

    async fn start_streaming_playback(&mut self, url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let path = self.storage.get_active_slot_path("current");
        let _ = tokio::fs::remove_file(&path).await;

        let direct_url = if is_direct_stream_url(url) {
            url.to_string()
        } else {
            resolve_youtube_stream_url(url).await?
        };

        let dl_path = path.clone();
        let cancel = Arc::new(AtomicBool::new(false));

        let download_cancel = cancel.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let download_handle = tokio::spawn(async move {
            let result = stream_http_to_file(&direct_url, &dl_path, download_cancel).await;
            let _ = tx.send(result);
        });

        if let Ok(Ok(Err(e))) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            rx,
        )
        .await
        {
            download_handle.abort();
            return Err(format!("stream download failed: {}", e).into());
        }

        {
            let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.pcm.clear();
            buf.read_pos = 0;
            buf.channels = 0;
            buf.file_sample_rate = 0;
        }

        let audio_buffer = self.audio_buffer.clone();
        let decode_path = path.clone();
        let decode_cancel = cancel.clone();
        let decode_handle = tokio::task::spawn_blocking(move || {
            if let Err(e) = streaming_decode_sync(decode_path, audio_buffer, decode_cancel) {
                error!("streaming decode task failed: {}", e);
            }
        });

        self.streaming = Some(StreamingPlayback {
            download_handle: Some(download_handle),
            decode_handle,
            cancel,
        });

        self.state = PlayerStateLabel::Playing;
        Ok(())
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.speed = self.speed;
        drop(buf);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume_val = volume.clamp(0.0, 1.0);
        if let Err(e) = self.set_pipewire_sink_input_volume(self.volume_val) {
            tracing::warn!("PipeWire volume control unavailable, using software gain: {e}");
            *self.volume.lock().unwrap_or_else(|e| e.into_inner()) = self.volume_val;
        }
    }

    fn set_pipewire_sink_input_volume(&self, volume: f32) -> Result<(), String> {
        let sink_input = Command::new("pactl")
            .arg("list")
            .arg("sink-inputs")
            .output()
            .map_err(|e| format!("pactl not available: {e}"))?;

        if !sink_input.status.success() {
            return Err("pactl list sink-inputs failed".into());
        }

        let output = String::from_utf8_lossy(&sink_input.stdout);
        let mut current_index: Option<u32> = None;
        let mut in_target = false;

        for line in output.lines() {
            let line = line.trim();
            if line.starts_with("Sink Input #") {
                current_index = line.rsplit('#').next().and_then(|s| s.trim().parse().ok());
                in_target = false;
            } else if line.starts_with("application.name =") || line.starts_with("node.name =") {
                in_target = line.contains("l337-audio-server");
            }

            if in_target && line.starts_with("volume:") {
                if let Some(index) = current_index {
                    let percentage = (volume * 100.0).round().clamp(0.0, 100.0);
                    let value = (volume * 65536.0).round().clamp(0.0, 65536.0) as u32;
                    let status = Command::new("pactl")
                        .arg("set-sink-input-volume")
                        .arg(index.to_string())
                        .arg(format!("{}%", percentage))
                        .status()
                        .map_err(|e| format!("pactl set-sink-input-volume failed: {e}"))?;

                    if !status.success() {
                        return Err(format!("pactl set-sink-input-volume exited with {}", status));
                    }
                    tracing::info!("Set PipeWire sink-input #{} volume to {}% ({})", index, percentage, value);
                    return Ok(());
                }
            }
        }

        Err("l337-audio-server sink input not found".into())
    }

    pub fn seek(&mut self, position: u64) {
        if self.streaming.is_some() {
            tracing::warn!("seek ignored during streaming playback");
            return;
        }

        let path = self.storage.get_active_slot_path("current");

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                error!("seek: cannot read {}: {}", path.display(), e);
                return;
            }
        };

        let decode =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || decode_to_pcm(bytes)));

        let (mut pcm, file_sample_rate, channels) = match decode {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                error!("seek: decode failed on {}: {:?}", path.display(), e);
                return;
            }
            Err(_) => {
                error!(
                    "seek: decoder panicked on {} (unsupported/corrupt container)",
                    path.display()
                );
                return;
            }
        };

        let target_orig_pos = (position as u64) * (file_sample_rate as u64);
        let start_sample = (target_orig_pos * channels as u64) as usize;
        if start_sample < pcm.len() {
            pcm = pcm.split_off(start_sample);
        }

        self.file_sample_rate = file_sample_rate;
        self.channels = channels;

        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.sample_rate
        };

        let playback_pcm =
            resample_interleaved(&pcm, channels, file_sample_rate, device_rate, self.speed);
        tracing::info!("seek: after resample: playback_pcm.len()={}", playback_pcm.len());

        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm = playback_pcm;
        buf.read_pos = 0;
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        drop(buf);

        self.position_sec = position;
        let remaining_frames = pcm.len() / channels as usize;
        self.duration_sec = Some((remaining_frames as f64 / file_sample_rate as f64).round() as u64);
        self.state = PlayerStateLabel::Playing;
    }

    pub async fn trigger_next(&mut self) {
        let prev_path = self.storage.get_active_slot_path("prev");
        let curr_path = self.storage.get_active_slot_path("current");
        let next_path = self.storage.get_active_slot_path("next");

        if !next_path.exists() {
            info!("Cannot trigger next: next.stream does not exist");
            return;
        }

        self.stop();

        if curr_path.exists() {
            let _ = fs::rename(&curr_path, &prev_path).await;
        }
        let _ = fs::rename(&next_path, &curr_path).await;

        self.load_and_play("current").await;
    }

    pub async fn trigger_previous(&mut self) {
        let prev_path = self.storage.get_active_slot_path("prev");
        let curr_path = self.storage.get_active_slot_path("current");
        let next_path = self.storage.get_active_slot_path("next");

        if !prev_path.exists() {
            info!("Cannot trigger previous: prev.stream does not exist");
            return;
        }

        self.stop();

        if curr_path.exists() {
            let _ = fs::rename(&curr_path, &next_path).await;
        }
        let _ = fs::rename(&prev_path, &curr_path).await;

        self.load_and_play("current").await;
    }

    pub async fn get_status(&self) -> PlayerStatus {
        let next_cached = self.storage.get_active_slot_path("next").exists();
        let prev_cached = self.storage.get_active_slot_path("prev").exists();
        let utilization = self.storage.get_total_size().await;

        let position_sec = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            if buf.pcm.is_empty() || buf.file_sample_rate == 0 {
                Some(self.position_sec)
            } else {
                let consumed = buf.read_pos as u64;
                let orig_frames = (consumed as f64 * buf.file_sample_rate as f64 * buf.speed as f64
                    / buf.sample_rate as f64) as u64;
                let buffer_sec = orig_frames / buf.file_sample_rate as u64;
                Some(self.position_sec + buffer_sec)
            }
        };

        PlayerStatus {
            state: self.state,
            volume: self.volume_val,
            speed: self.speed,
            current_track: self.current_track.clone(),
            disk_pool_utilization_bytes: utilization,
            next_cached,
            prev_cached,
            position_sec,
            duration_sec: self.duration_sec,
            audio_available: self.stream.is_some(),
        }
    }
}

pub(crate) fn decode_to_pcm(
    bytes: Vec<u8>,
) -> Result<(Vec<f32>, u32, u16), Box<dyn std::error::Error + Send + Sync>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;

    let source = std::io::Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(source), Default::default());

    let hint = symphonia::core::probe::Hint::new();
    let format_opts = Default::default();
    let metadata_opts = Default::default();
    let decoder_opts = Default::default();

    let probed =
        symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

    let mut format = probed.format;
    let track = format.default_track().ok_or("no default track")?;
    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    tracing::info!(?codec_params, codec_id = ?codec_params.codec, "decode_to_pcm: probing result");
    let mut decoder = match symphonia::default::get_codecs().make(&codec_params, &decoder_opts) {
        Ok(decoder) => decoder,
        Err(e) => {
            tracing::error!(?codec_params, error = ?e, "decode_to_pcm: decoder creation failed");
            return Err(format!("decoder init: {e}").into());
        }
    };

    let mut sample_buf = None;
    let mut pcm = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                if sample_buf.is_none() {
                    let spec = *audio_buf.spec();
                    let duration = audio_buf.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    pcm.extend_from_slice(buf.samples());
                }
            }
            Err(_) => break,
        }
    }

    let sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.unwrap_or_default().count() as u16;
    Ok((pcm, sample_rate, channels))
}

pub(crate) fn resample_interleaved(
    input: &[f32],
    channels: u16,
    in_rate: u32,
    out_rate: u32,
    speed: f32,
) -> Vec<f32> {
    if input.is_empty() || speed <= 0.0 {
        return Vec::new();
    }

    let channels = channels as usize;
    let params = SincInterpolationParameters {
        sinc_len: 1024,
        f_cutoff: 0.95,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk_size = 4096;

    let effective_in_rate = (in_rate as f64) * (speed as f64);
    let ratio = out_rate as f64 / effective_in_rate;

    let mut resampler = SincFixedIn::<f32>::new(ratio, 10.0, params, chunk_size, channels).unwrap();

    let mut input_planar = vec![Vec::new(); channels];
    for chunk in input.chunks(channels) {
        for (ch, &sample) in chunk.iter().enumerate() {
            input_planar[ch].push(sample);
        }
    }

    let mut output = Vec::new();
    let mut input_offset = 0;
    loop {
        let frames_needed = resampler.input_frames_next();
        if frames_needed == 0 {
            break;
        }
        if input_offset + frames_needed > input_planar[0].len() {
            if input_offset < input_planar[0].len() {
                let _remaining = &input_planar[0].len() - input_offset;
                let mut remaining_input: Vec<Vec<f32>> = input_planar
                    .iter()
                    .map(|v| v[input_offset..].to_vec())
                    .collect();
                let output_planar = resampler.process_partial(Some(&remaining_input), None).unwrap();
                for ch in 0..channels {
                    output.extend_from_slice(&output_planar[ch]);
                }
            }
            break;
        }

        let mut chunk_input: Vec<Vec<f32>> = input_planar
            .iter()
            .map(|v| v[input_offset..input_offset + frames_needed].to_vec())
            .collect();
        let output_planar = resampler.process(&chunk_input, None).unwrap();
        for ch in 0..channels {
            output.extend_from_slice(&output_planar[ch]);
        }
        input_offset += frames_needed;
    }
    output
}

#[cfg(test)]
pub(crate) static DOWNLOAD_STREAM_INVOKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub async fn download_stream(
    url: &str,
    dest: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    #[cfg(test)]
    {
        DOWNLOAD_STREAM_INVOKED.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    let local_path: Option<PathBuf> = if let Some(path) = url.strip_prefix("file://") {
        Some(PathBuf::from(path))
    } else if !url.contains("://") && PathBuf::from(url).is_absolute() {
        Some(PathBuf::from(url))
    } else {
        None
    };

    if let Some(src) = local_path {
        if src.exists() {
            tokio::fs::copy(&src, dest).await?;
            return Ok(());
        }
    }

    if is_youtube_url(url) && !is_direct_stream_url(url) {
        if !yt_dlp_available() {
            return Err("yt-dlp is not installed. Install yt-dlp to play/download YouTube URLs.".into());
        }
        return download_via_ytdlp(url, dest)
            .await
            .map_err(|e| format!("yt-dlp download failed for {url}: {e}").into());
    }

    let response = reqwest::Client::builder()
        .user_agent(concat!("l337-audio-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("reqwest client build failed: {e}"))?
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("fetch failed ({}): {}", response.status(), url).into());
    }

    if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let ct = ct.to_str().unwrap_or("").to_lowercase();
        if ct.contains("text/html") {
            return Err(format!(
                "refusing to download HTML response for {url} (content-type {ct})"
            )
            .into());
        }
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .await?;

    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;

    if let Ok(bytes) = tokio::fs::read(dest).await {
        if bytes.len() >= 5
            && (&bytes[0..5] == b"<!doc" || &bytes[0..5] == b"<html" || &bytes[0..5] == b"<?xml")
        {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(
                format!("downloaded content for {url} appears to be markup, not audio").into(),
            );
        }
    }
    Ok(())
}

fn is_youtube_url(url: &str) -> bool {
    url.contains("youtube.com/watch")
        || url.contains("youtu.be/")
        || url.contains("youtube.com/shorts/")
        || url.contains("googlevideo.com/videoplayback")
}

fn is_direct_stream_url(url: &str) -> bool {
    url.contains("googlevideo.com/videoplayback")
}

fn detect_youtube_block(stderr: &str) -> Option<YouTubeBlockType> {
    let lower = stderr.to_lowercase();
    if lower.contains("http error 429") || lower.contains("429") {
        return Some(YouTubeBlockType::RateLimit);
    }
    if lower.contains("captcha") || lower.contains("playercaptchaviewmodel") {
        return Some(YouTubeBlockType::Captcha);
    }
    if lower.contains("403") || lower.contains("cloudflare") {
        return Some(YouTubeBlockType::BotDetected);
    }
    if lower.contains("no video formats found") || lower.contains("skipping player response") {
        return Some(YouTubeBlockType::IpBlocked);
    }
    None
}

fn yt_dlp_available() -> bool {
    Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn download_via_ytdlp(
    url: &str,
    dest: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !yt_dlp_available() {
        return Err("yt-dlp is not installed. Install yt-dlp to download YouTube URLs.".into());
    }

    let _ = tokio::fs::remove_file(dest).await;

    let output = TokioCommand::new("yt-dlp")
        .arg("--no-config")
        .arg("--no-warnings")
        .arg("-f")
        .arg("bestaudio")
        .arg("--no-playlist")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(block_type) = detect_youtube_block(&stderr) {
        return Err(YouTubeError {
            block_type,
            message: format!("yt-dlp block detected: {:?}", block_type),
        }
        .into());
    }

    if !output.status.success() {
        return Err(format!("yt-dlp exited with {}", output.status).into());
    }
    let meta = tokio::fs::metadata(dest).await?;
    if meta.len() == 0 {
        return Err("yt-dlp produced an empty file".into());
    }
    Ok(())
}

async fn resolve_youtube_stream_url(url: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if !yt_dlp_available() {
        return Err("yt-dlp is not installed. Install yt-dlp to resolve YouTube URLs.".into());
    }

    let output = TokioCommand::new("yt-dlp")
        .arg("--no-config")
        .arg("--no-warnings")
        .arg("-g")
        .arg("-f")
        .arg("bestaudio")
        .arg("--no-playlist")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(block_type) = detect_youtube_block(&stderr) {
        return Err(YouTubeError {
            block_type,
            message: format!("yt-dlp block detected: {:?}", block_type),
        }
        .into());
    }

    if !output.status.success() {
        return Err("yt-dlp -g failed".into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stream_url = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or("yt-dlp -g returned empty output")?
        .trim()
        .to_string();

    Ok(stream_url)
}

async fn stream_http_to_file(
    url: &str,
    dest: &PathBuf,
    cancel: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let response = reqwest::Client::builder()
        .user_agent(concat!("l337-audio-server/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("reqwest client build failed: {e}"))?
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!("fetch failed ({}): {}", response.status(), url).into());
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .await?;

    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let chunk = item?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

/// Adapter that lets Symphonia read from a standard file as a `MediaSource`.
struct FileSource {
    file: std::fs::File,
}

impl std::io::Read for FileSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl std::io::Seek for FileSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for FileSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.file.metadata().ok().map(|m| m.len())
    }
}

/// Synchronous decode loop for streaming playback.
///
/// Waits for the download to produce data, then incrementally decodes packets
/// and appends PCM into `audio_buffer` for the audio callback to consume.
fn streaming_decode_sync(
    path: PathBuf,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut file_size: u64 = 0;
    let mut stall_cycles: u32 = 0;

    for _ in 0..120 {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Ok(meta) = std::fs::metadata(&path) {
            file_size = meta.len();
            if file_size > 0 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    if file_size == 0 {
        return Err("stream file never received data".into());
    }

    let mut probed = None;
    for attempt in 0..60 {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let source = FileSource { file };
        let mss = symphonia::core::io::MediaSourceStream::new(Box::new(source), Default::default());
        let hint = symphonia::core::probe::Hint::new();
        match symphonia::default::get_probe().format(
            &hint,
            mss,
            &Default::default(),
            &Default::default(),
        ) {
            Ok(p) => {
                probed = Some(p);
                break;
            }
            Err(_) if attempt < 59 => {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => return Err(format!("format probe failed after retries: {e}")),
        }
    }

    let probed = probed.unwrap();
    let mut format = probed.format;
    let track_id;
    let codec_params;
    let mut decoder_opts = Default::default();

    let audio_track = format.tracks().iter().find(|t| {
        let ct = t.codec_params.codec;
        ct == symphonia::core::codecs::CODEC_TYPE_AAC
            || ct == symphonia::core::codecs::CODEC_TYPE_MP3
            || ct == symphonia::core::codecs::CODEC_TYPE_OPUS
            || ct == symphonia::core::codecs::CODEC_TYPE_VORBIS
            || ct == symphonia::core::codecs::CODEC_TYPE_FLAC
            || ct == symphonia::core::codecs::CODEC_TYPE_ALAC
            || ct == symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
    });

    let track = if let Some(t) = audio_track {
        t
    } else {
        format
            .default_track()
            .ok_or("no default track in stream")?
    };
    track_id = track.id;
    codec_params = track.codec_params.clone();

    let file_sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2).max(1);

    {
        let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ab.channels = channels;
        ab.file_sample_rate = file_sample_rate;
    }

    tracing::info!(?codec_params, codec_id = ?codec_params.codec, "streaming_decode_sync: selected track codec params");
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &decoder_opts)
        .map_err(|e| {
            tracing::error!(?codec_params, error = ?e, "streaming_decode_sync: decoder creation failed");
            format!("decoder init: {e}")
        })?;

    let device_rate = {
        let ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ab.sample_rate
    };
    let speed = {
        let ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ab.speed
    };

    let mut resampler = if file_sample_rate != device_rate || speed != 1.0 {
        let channels = channels as usize;
        let params = SincInterpolationParameters {
            sinc_len: 1024,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        };
        let chunk_size = 4096;
        let effective_in_rate = (file_sample_rate as f64) * (speed as f64);
        let ratio = device_rate as f64 / effective_in_rate;
        Some(SincFixedIn::<f32>::new(ratio, 10.0, params, chunk_size, channels).unwrap())
    } else {
        None
    };

    let mut sample_buf = None;
    let mut last_file_size = file_size;

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }

        match format.next_packet() {
            Ok(packet) => {
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(audio_buf) => {
                        let spec = *audio_buf.spec();
                        if sample_buf.is_none() {
                            sample_buf = Some(symphonia::core::audio::SampleBuffer::<f32>::new(
                                audio_buf.capacity() as u64,
                                spec,
                            ));
                        }

                        if let Some(buf) = &mut sample_buf {
                            buf.copy_interleaved_ref(audio_buf);
                            let samples = buf.samples();
                            let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());

                            if let Some(ref mut resampler) = resampler {
                                let mut input_planar = vec![Vec::new(); channels as usize];
                                for chunk in samples.chunks(channels as usize) {
                                    for (ch, &sample) in chunk.iter().enumerate() {
                                        input_planar[ch].push(sample);
                                    }
                                }

                                if let Ok(output_planar) = resampler.process(&input_planar, None) {
                                    let output_len = output_planar[0].len();
                                    let mut output = Vec::with_capacity(output_len * channels as usize);
                                    for i in 0..output_len {
                                        for ch in 0..channels as usize {
                                            output.push(output_planar[ch][i]);
                                        }
                                    }
                                    ab.pcm.extend_from_slice(&output);
                                }
                            } else {
                                ab.pcm.extend_from_slice(samples);
                            }
                        }

                        stall_cycles = 0;
                    }
                    Err(symphonia::core::errors::Error::IoError(_)) => {
                        stall_cycles += 1;
                        if stall_cycles > 20 {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    Err(symphonia::core::errors::Error::ResetRequired) => {
                        let decoder_opts = Default::default();
                        decoder = symphonia::default::get_codecs()
                            .make(&codec_params, &decoder_opts)
                            .map_err(|e| format!("decoder reset: {e}"))?;
                    }
                    Err(e) => {
                        error!("streaming decode error: {:?}", e);
                        break;
                    }
                }
            }
            Err(symphonia::core::errors::Error::IoError(_)) => {
                let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if current_size > last_file_size {
                    last_file_size = current_size;
                    stall_cycles = 0;
                    std::thread::sleep(std::time::Duration::from_millis(200));
                } else if stall_cycles > 20 {
                    break;
                } else {
                    stall_cycles += 1;
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
            Err(e) => {
                error!("streaming packet error: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod send_tests {
    fn assert_send<T: Send>() {}
    #[test]
    fn test_player_engine_send() {
        assert_send::<crate::player::engine::PlayerEngine>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Track;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_detect_youtube_block_rate_limit() {
        assert_eq!(
            detect_youtube_block("HTTP Error 429 Too Many Requests"),
            Some(YouTubeBlockType::RateLimit)
        );
        assert_eq!(detect_youtube_block("yt-dlp: 429"), Some(YouTubeBlockType::RateLimit));
    }

    #[test]
    fn test_detect_youtube_block_captcha() {
        assert_eq!(
            detect_youtube_block("playerCaptchaViewModel"),
            Some(YouTubeBlockType::Captcha)
        );
        assert_eq!(
            detect_youtube_block("Captcha required, please solve"),
            Some(YouTubeBlockType::Captcha)
        );
    }

    #[test]
    fn test_detect_youtube_block_bot_detected() {
        assert_eq!(
            detect_youtube_block("HTTP Error 403 Forbidden"),
            Some(YouTubeBlockType::BotDetected)
        );
        assert_eq!(
            detect_youtube_block("cloudflare protection detected"),
            Some(YouTubeBlockType::BotDetected)
        );
    }

    #[test]
    fn test_detect_youtube_block_ip_blocked() {
        assert_eq!(
            detect_youtube_block("No video formats found"),
            Some(YouTubeBlockType::IpBlocked)
        );
        assert_eq!(
            detect_youtube_block("Skipping player response"),
            Some(YouTubeBlockType::IpBlocked)
        );
    }

    #[test]
    fn test_detect_youtube_block_no_match() {
        assert_eq!(detect_youtube_block("some random error"), None);
        assert_eq!(detect_youtube_block("yt-dlp: unknown error"), None);
        assert_eq!(detect_youtube_block(""), None);
    }

    #[test]
    fn test_youtube_block_type_as_str() {
        assert_eq!(YouTubeBlockType::RateLimit.as_str(), "rate_limit");
        assert_eq!(YouTubeBlockType::Captcha.as_str(), "captcha");
        assert_eq!(YouTubeBlockType::BotDetected.as_str(), "bot_detected");
        assert_eq!(YouTubeBlockType::IpBlocked.as_str(), "ip_blocked");
    }

    #[test]
    fn test_engine_error_display() {
        let yt_err = YouTubeError {
            block_type: YouTubeBlockType::RateLimit,
            message: "rate limited".to_string(),
        };
        let engine_err = EngineError::YouTube(yt_err);
        assert_eq!(format!("{}", engine_err), "rate limited");

        let other_err = EngineError::Other("generic failure".to_string());
        assert_eq!(format!("{}", other_err), "generic failure");
    }

    #[tokio::test]
    async fn test_play_track_short_circuits_on_youtube_block() {
        let dir = std::env::temp_dir().join("l337-block-test");
        let _ = std::fs::create_dir_all(&dir);
        let fake_ytdlp = dir.join("yt-dlp");
        let script = r#"#!/bin/bash
if [ "$1" = "--version" ]; then
    echo "fake yt-dlp"
    exit 0
fi
echo "HTTP Error 429 Too Many Requests" >&2
exit 1
"#;
        std::fs::write(&fake_ytdlp, script).unwrap();
        let _ = std::fs::set_permissions(&fake_ytdlp, std::fs::Permissions::from_mode(0o755));

        let original_path = std::env::var("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", format!("{}:{}", dir.display(), original_path)); }
        DOWNLOAD_STREAM_INVOKED.store(false, Ordering::SeqCst);

        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let mut engine = PlayerEngine::new_dummy(storage);

        let track = Track {
            track_id: "test".to_string(),
            stream_url: "https://www.youtube.com/watch?v=test".to_string(),
            title: None,
            artist: None,
            duration: None,
        };

        let result = engine.play_track(track).await;

        unsafe { std::env::set_var("PATH", original_path); }

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::YouTube(yt_err) => {
                assert!(matches!(yt_err.block_type, YouTubeBlockType::RateLimit));
            }
            ref e => panic!("Expected EngineError::YouTube, got: {:?}", e),
        }
        assert!(
            !DOWNLOAD_STREAM_INVOKED.load(Ordering::SeqCst),
            "download_stream should not be called when start_streaming_playback returns a block"
        );
    }
}
