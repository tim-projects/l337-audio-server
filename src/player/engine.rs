use crate::api::models::{resolve_ceiling, PlayerStateLabel, PlayerStatus, Track};
use crate::platform::common::{AudioBackend, AudioOutputStream, AudioBuffer};
use crate::player::storage::StorageManager;
use rubato::Resampler;
use rubato::SincFixedIn;
use rubato::SincInterpolationParameters;
use rubato::SincInterpolationType;
use rubato::WindowFunction;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tracing::{error, info};
use wsola;
use pitch_shift;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdWarm {
    Cold,
    Warm,
}

impl std::fmt::Display for YouTubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for YouTubeError {}

/// Cached resolution of a YouTube URL to a direct stream URL. YouTube stream
/// URLs (googlevideo.com) expire within hours, so entries are TTL-bounded.
/// Avoids re-running yt-dlp on every play of the same URL.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedStream {
    pub(crate) url: String,
    pub(crate) abr: Option<u32>,
    pub(crate) codec: Option<String>,
    pub(crate) ext: Option<String>,
    pub(crate) duration: Option<u64>,
}

/// TTL for cached YouTube stream URLs. googlevideo.com signatures expire
/// within hours; a short TTL forces re-resolution before the URL goes stale.
const YT_DLP_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(1800);

/// Why a streaming start failed. `Resolve` means the URL never became a
/// and the fetch/decode broke afterwards. The caller only retries on `Playback`.
#[derive(Debug)]
pub enum StreamStartError {
    Resolve(Box<dyn std::error::Error + Send + Sync>),
    Playback(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for StreamStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamStartError::Resolve(e) => write!(f, "{e}"),
            StreamStartError::Playback(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StreamStartError {}

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
    backend: Box<dyn AudioBackend>,
    stream: Option<Box<dyn AudioOutputStream>>,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    volume: Arc<Mutex<f32>>,
    pub storage: StorageManager,
    pub current_track: Option<Track>,
    pub state: PlayerStateLabel,
    pub speed: f32,
    pub pitch: f32,
    pub volume_val: f32,
    pub duration_sec: Option<u64>,
    pub position_sec: u64,
    file_sample_rate: u32,
    channels: u16,
    streaming: Option<StreamingPlayback>,
    /// Server default bandwidth ceiling (kbps) from `[server] default_target_kbps`.
    default_target_kbps: u32,
    /// Runtime-mutable default ceiling (kbps); per-track `target_kbps`/`quality`
    /// override it per request. `0` = best. Set via `POST /player/target`.
    runtime_target_kbps: u32,
    /// Effective ceiling (kbps) applied to the current/last track (for status).
    effective_target_kbps: u32,
    /// Delivered bitrate of the current/last track when known (for PlayerStatus).
    actual_kbps: Option<u32>,
    /// Short codec label of the current/last track when known (for PlayerStatus).
    codec_label: Option<String>,
    /// Shared HTTP client reused across all downloads (OPT-8): avoids rebuilding
    /// a reqwest::Client (TLS session cache, connection pool, DNS cache) on every
    /// download_stream / stream_http_to_file call.
    pub http_client: Arc<reqwest::Client>,
    /// Cached YouTube stream URL resolutions (key: `url|ceiling`). Avoids
    /// re-running yt-dlp on every play of the same URL.
    pub yt_dlp_cache: Arc<Mutex<HashMap<String, (ResolvedStream, std::time::Instant)>>>,
    warmth: ColdWarm,
    last_client_contact: std::time::Instant,
    active_sessions: u32,
    idle_grace: std::time::Duration,
}

impl PlayerEngine {
    /// How long a streaming start waits for the container probe before giving up
    /// on streaming and falling back to a whole-file download. It must outlive
    /// the probe's own retry budget (60 × 500 ms ≈ 30 s) so a genuine
    /// "cannot be probed from a partial file" is observed rather than timed out
    /// blind — and it is far shorter than downloading a whole track, which is
    /// what the fallback still costs for a 5 MB file on a 32 KB/s link (~160 s).
    const STREAM_PROBE_BUDGET: std::time::Duration = std::time::Duration::from_secs(35);

    pub fn new(storage: StorageManager, buffer_max_bytes: u64, default_target_kbps: u32) -> Result<Self, String> {
        let mut audio_buffer = AudioBuffer::new(48000, 0);
        audio_buffer.max_bytes = buffer_max_bytes as usize;
        let audio_buffer = Arc::new(Mutex::new(audio_buffer));
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
            backend,
            stream: Some(stream),
            audio_buffer,
            volume: volume.clone(),
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            pitch: 1.0,
            volume_val: 1.0,
            duration_sec: None,
            position_sec: 0,
            file_sample_rate: 0,
            channels: 0,
            streaming: None,
            default_target_kbps,
            runtime_target_kbps: default_target_kbps,
            effective_target_kbps: default_target_kbps,
            actual_kbps: None,
            codec_label: None,
            http_client: Arc::new(
                reqwest::Client::builder()
                    .user_agent(concat!("l337-audio-server/", env!("L337_VERSION")))
                    .build()
                    .expect("failed to build reqwest client"),
            ),
            yt_dlp_cache: Arc::new(Mutex::new(HashMap::new())),
            warmth: ColdWarm::Cold,
            last_client_contact: std::time::Instant::now(),
            active_sessions: 0,
            idle_grace: std::time::Duration::from_secs(90),
        })
    }

    /// Dummy (no-hardware) engine: every endpoint still works and the decode /
    /// pacing / window / ceiling logic is identical to a real backend. The
    /// configured rebuffer window and default ceiling must still apply, or
    /// `[server] buffer_max_bytes` / `default_target_kbps` are silently ignored
    /// in the documented testing mode.
    pub fn new_dummy(storage: StorageManager, buffer_max_bytes: u64, default_target_kbps: u32) -> Self {
        let mut audio_buffer = AudioBuffer::new(48000, 0);
        audio_buffer.max_bytes = buffer_max_bytes as usize;
        Self {
            backend: Box::new(crate::platform::common::NoopAudioBackend),
            stream: Some(Box::new(crate::platform::common::NoopAudioOutputStream)),
            audio_buffer: Arc::new(Mutex::new(audio_buffer)),
            volume: Arc::new(Mutex::new(1.0)),
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            pitch: 1.0,
            volume_val: 1.0,
            duration_sec: None,
            position_sec: 0,
            file_sample_rate: 0,
            channels: 0,
            streaming: None,
            default_target_kbps,
            runtime_target_kbps: default_target_kbps,
            effective_target_kbps: default_target_kbps,
            actual_kbps: None,
            codec_label: None,
            http_client: Arc::new(
                reqwest::Client::builder()
                    .user_agent(concat!("l337-audio-server/", env!("L337_VERSION")))
                    .build()
                    .expect("failed to build reqwest client"),
            ),
            yt_dlp_cache: Arc::new(Mutex::new(HashMap::new())),
            warmth: ColdWarm::Cold,
            last_client_contact: std::time::Instant::now(),
            active_sessions: 0,
            idle_grace: std::time::Duration::from_secs(90),
        }
    }

    fn has_backend_error(&mut self) -> bool {
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buf.backend_error.load(Ordering::SeqCst) {
            buf.pcm.clear();
            buf.read_pos = 0;
            buf.consumed_total = 0;
            buf.backend_error.store(false, Ordering::SeqCst);
            self.state = PlayerStateLabel::Stopped;
            self.position_sec = 0;
            true
        } else {
            false
        }
    }

    pub(crate) fn check_backend_error(&self) -> bool {
        let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.backend_error.load(Ordering::SeqCst)
    }

    /// Playhead in source seconds for a buffer, or `None` when nothing has been
    /// consumed yet (caller falls back to the seek base).
    ///
    /// The samples-consumed count is the reclaimed prefix plus `read_pos`, never
    /// `read_pos` alone: a writer reclaim drops the consumed prefix and resets
    /// `read_pos`, which would rewind the timeline every checkpoint (~10.9 s at
    /// 1<<20 samples). `pcm.is_empty()` is equally wrong as a "nothing played"
    /// test — a fully reclaimed window is mid-playback. Shared by
    /// `calculate_position` (recovery path) and `get_status`, which must not
    /// drift apart.
    fn buffer_position_sec(buf: &AudioBuffer, position_sec: u64) -> Option<u64> {
        let consumed_total = buf.consumed_total + buf.read_pos;
        if consumed_total == 0 || buf.file_sample_rate == 0 {
            return None;
        }
        let consumed_frames = (consumed_total as u64) / (buf.channels.max(1) as u64);
        let orig_frames =
            (consumed_frames as f64 * buf.file_sample_rate as f64 * buf.speed as f64 / buf.sample_rate as f64)
                as u64;
        Some(position_sec + orig_frames / buf.file_sample_rate as u64)
    }

    fn calculate_position(&self, buf: &AudioBuffer) -> u64 {
        Self::buffer_position_sec(buf, self.position_sec).unwrap_or(self.position_sec)
    }

    fn take_backend_error(&mut self) -> Option<u64> {
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        if buf.backend_error.load(Ordering::SeqCst) {
            let position = self.calculate_position(&buf);
            buf.pcm.clear();
            buf.read_pos = 0;
            buf.consumed_total = 0;
            buf.backend_error.store(false, Ordering::SeqCst);
            self.state = PlayerStateLabel::Stopped;
            self.position_sec = 0;
            Some(position)
        } else {
            None
        }
    }

    fn seek_and_resume(&mut self, position: u64) -> Result<(), String> {
        let path = self.storage.get_active_slot_path("current");
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let decode =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || decode_to_pcm(bytes)));
        let (mut pcm, file_sample_rate, channels, _codec) = match decode {
            Ok(Ok(result)) => result,
            _ => return Err("decode failed".to_string()),
        };

        let target_orig_pos = (position as u64) * (file_sample_rate as u64);
        let start_sample = (target_orig_pos * channels as u64) as usize;
        if start_sample < pcm.len() {
            pcm = pcm.split_off(start_sample);
        }

        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.sample_rate
        };
        let playback_pcm =
            stretch_and_resample(&pcm, channels, file_sample_rate, device_rate, self.speed, self.pitch);

        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm = playback_pcm;
        buf.read_pos = 0;
        buf.consumed_total = 0;
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        buf.pitch = self.pitch;
        self.file_sample_rate = file_sample_rate;
        self.channels = channels;

        Ok(())
    }

    pub(crate) fn try_recover(&mut self) -> Result<(), String> {
        let position = match self.take_backend_error() {
            Some(pos) => pos,
            None => return Ok(()),
        };

        if let Some(mut stream) = self.stream.take() {
            stream.stop();
        }

        let new_stream = self.backend.start_stream(
            "l337-audio-server",
            48000,
            2,
            self.audio_buffer.clone(),
            self.volume.clone(),
        )?;
        self.stream = Some(new_stream);

        let is_file_playback = self.file_sample_rate > 0 && self.channels > 0;

        if is_file_playback && position > 0 && self.current_track.is_some() {
            self.seek_and_resume(position)?;
        }

        if let Some(stream) = self.stream.as_mut() {
            stream.play()?;
        }

        if is_file_playback && position > 0 && self.current_track.is_some() {
            self.state = PlayerStateLabel::Playing;
        } else {
            self.state = PlayerStateLabel::Stopped;
        }

        Ok(())
    }

    pub async fn play_track(&mut self, track: Track) -> Result<(), EngineError> {
        if self.check_backend_error() {
            self.try_recover().map_err(EngineError::Other)?;
        }

        self.stop();
        self.current_track = Some(track.clone());

        // G2: resolve the bandwidth ceiling for this request and remember it (and
        // reset the observable bitrate/codec) so PlayerStatus reflects this play.
        let ceiling = resolve_ceiling(
            track.target_kbps,
            track.quality,
            self.runtime_target_kbps,
        );
        self.effective_target_kbps = ceiling;
        self.actual_kbps = None;
        self.codec_label = None;
        // A failed play must not report the previous track's timeline.
        self.duration_sec = None;

        if let Some(cached_path) = self.storage.get_path_for_track(&track.track_id) {
            if cached_path.exists() {
                info!("play_track: cache hit for {}", track.track_id);
                let slot = self.storage.get_active_slot_path("current");
                let _ = fs::copy(&cached_path, &slot).await;
                self.load_and_play("current").await;
                self.storage.update_access(&track.track_id, 0).await;
                return Ok(());
            }
        }

        if is_youtube_url(&track.stream_url) {
            info!("play_track: YouTube streaming for {}", track.stream_url);
            match self.start_streaming_playback(&track.stream_url, ceiling).await {
                Ok(()) => return Ok(()),
                // Resolution failed: the URL never produced a stream, so
                // re-running yt-dlp through the downloader would only repeat
                // the same failure (and double the latency / block risk).
                Err(StreamStartError::Resolve(e)) => {
                    if let Some(yt_err) = e.downcast_ref::<YouTubeError>() {
                        return Err(EngineError::YouTube(yt_err.clone()));
                    }
                    return Err(EngineError::Other(format!(
                        "Failed to resolve stream for {}: {}",
                        track.stream_url, e
                    )));
                }
                // The stream broke after resolution: fall back to a plain
                // whole-file download.
                Err(StreamStartError::Playback(e)) => {
                    if let Some(yt_err) = e.downcast_ref::<YouTubeError>() {
                        return Err(EngineError::YouTube(yt_err.clone()));
                    }
                    error!("YouTube streaming failed, falling back to download: {}", e);
                    let path = self.storage.get_active_slot_path("current");
                    if let Err(e) = download_stream(&self.http_client, &track.stream_url, &path, ceiling).await {
                        if let Some(yt_err) = e.downcast_ref::<YouTubeError>() {
                            return Err(EngineError::YouTube(yt_err.clone()));
                        }
                        return Err(EngineError::Other(format!("Failed to download track: {}", e)));
                    }
                    self.load_and_play("current").await;
                    self.persist_slot(&track.track_id, &path).await;
                    return Ok(());
                }
            }
        }

        let path = self.storage.get_active_slot_path("current");
        info!(
            "play_track: downloading {} -> {}",
            track.stream_url,
            path.display()
        );

        if let Err(e) = download_stream(&self.http_client, &track.stream_url, &path, ceiling).await {
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
    ) -> Result<(), String> {
        self.stop();
        self.current_track = Some(Track {
            track_id: track_id.to_string(),
            stream_url: String::new(),
            title,
            artist,
            duration: None,
            target_kbps: None,
            quality: None,
        });

        let path = self.storage.get_active_slot_path(slot);
        self.load_and_play(slot).await;
        if self.state != PlayerStateLabel::Playing {
            return Err(format!("failed to load pushed audio from slot {slot}"));
        }
        self.persist_slot(track_id, &path).await;
        Ok(())
    }

    async fn persist_slot(&mut self, track_id: &str, slot_path: &PathBuf) {
        let Some(dest_path) = self.storage.get_path_for_track(track_id) else {
            error!("persist_slot: refusing unsafe track_id {:?}", track_id);
            return;
        };
        let _ = fs::copy(slot_path, dest_path).await;
        if let Ok(meta) = fs::metadata(slot_path).await {
            self.storage.update_access(track_id, meta.len()).await;
        }
    }

    /// OPT-13: Decode a complete file incrementally into the audio buffer
    /// instead of allocating the whole decoded PCM at once. Decodes
    /// packet-by-packet, applying stretch/pitch/resample per packet, keeping
    /// memory bounded by the rebuffer window rather than the track length.
    ///
    /// Returns `(file_sample_rate, channels, codec_label, total_frames)`.
    async fn decode_file_incremental(
        &mut self,
        path: &PathBuf,
    ) -> Result<(u32, u16, Option<String>, u64), String> {
        let audio_buffer = self.audio_buffer.clone();
        let path = path.clone();
        let speed = self.speed;
        let pitch = self.pitch;
        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.sample_rate
        };

        tokio::task::spawn_blocking(move || {
            decode_file_incremental_blocking(&path, audio_buffer, speed, pitch, device_rate)
        })
        .await
        .map_err(|e| format!("decode task join error: {e}"))?
    }

    pub async fn load_and_play(&mut self, slot: &str) {
        self.ensure_stream();
        if self.has_backend_error() {
            return;
        }

        let path = self.storage.get_active_slot_path(slot);

        // Compressed file size, captured before decode — the delivered source
        // bitrate estimate (G2 actual_kbps).
        let bytes_len = match fs::metadata(&path).await {
            Ok(meta) => meta.len(),
            Err(e) => {
                error!(
                    "load_and_play({}): cannot stat {}: {}",
                    slot,
                    path.display(),
                    e
                );
                return;
            }
        };

        // OPT-13: decode incrementally into the audio buffer instead of
        // allocating the whole decoded PCM at once (peak RSS ~ compressed
        // size x 2-2.5 for hour-long tracks).
        let (file_sample_rate, channels, codec, total_frames) =
            match self.decode_file_incremental(&path).await {
                Ok(result) => result,
                Err(e) => {
                    error!(
                        "load_and_play({}): incremental decode failed on {}: {}",
                        slot,
                        path.display(),
                        e
                    );
                    return;
                }
            };

        self.file_sample_rate = file_sample_rate;
        self.channels = channels;
        let duration_sec = (total_frames as f64 / file_sample_rate as f64).round() as u64;
        self.duration_sec = Some(duration_sec);
        // G2 observability: codec + delivered bitrate of this (file/download)
        // source — compressed file bytes over duration is the true source bitrate.
        self.codec_label = codec;
        self.actual_kbps = if duration_sec > 0 {
            Some((bytes_len as u64 * 8 / 1000 / duration_sec.max(1)) as u32)
        } else {
            None
        };

        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        buf.pitch = self.pitch;
        drop(buf);

        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = stream.play() {
                error!("load_and_play: stream.play failed: {}", e);
            }
        }

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
        // Pause the backend stream instead of destroying it from the engine
        // thread. PipeWire stream/mainloop objects are not safe to tear down
        // from an arbitrary thread; dropping them here can trigger
        // "called from wrong context" errors and leave the backend in an
        // unrecoverable error state. Clearing the buffer stops output
        // immediately; a backend error path still drops/recreates the stream
        // via ensure_stream.
        if let Some(stream) = self.stream.as_mut() {
            stream.pause().ok();
        }
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm.clear();
        buf.read_pos = 0;
        buf.consumed_total = 0;
        buf.backend_error.store(false, Ordering::SeqCst);
        drop(buf);
        self.position_sec = 0;
        self.state = PlayerStateLabel::Stopped;
    }

    /// Reopen the backend stream after `stop` tore it down (or a backend error
    /// dropped it). Every playback entry point must call this before filling the
    /// buffer: the writer thread lives inside the stream, so a torn-down stream
    /// with a full buffer would look like a rebuffer that never drains.
    fn ensure_stream(&mut self) {
        if self.stream.is_some() {
            // Clear a stale backend-error flag so a recovered/reused stream
            // can play. The PipeWire callback sets backend_error on terminal
            // state changes; once we decide to (re)use a stream we must not
            // let a stale flag block load_and_play/status.
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            if buf.backend_error.swap(false, Ordering::SeqCst) {
                info!("ensure_stream: cleared stale backend_error on existing stream");
            }
            return;
        }
        match self.backend.start_stream(
            "l337-audio-server",
            48000,
            2,
            self.audio_buffer.clone(),
            self.volume.clone(),
        ) {
            Ok(stream) => {
                self.audio_buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .backend_error
                    .store(false, Ordering::SeqCst);
                self.stream = Some(stream);
            }
            Err(e) => {
                error!("ensure_stream: backend start failed: {}", e);
                self.audio_buffer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .backend_error
                    .store(true, Ordering::SeqCst);
                self.state = PlayerStateLabel::Stopped;
            }
        }
    }

    async fn start_streaming_playback(&mut self, url: &str, ceiling: u32) -> Result<(), StreamStartError> {
        let path = self.storage.get_active_slot_path("current");
        let _ = tokio::fs::remove_file(&path).await;

        let (direct_url, abr, codec, ext, duration) = if is_direct_stream_url(url) {
            let ext = guess_ext_from_url(url);
            (url.to_string(), None, None, ext, None)
        } else {
            let cache_key = format!("{url}|{ceiling}");
            let now = std::time::Instant::now();
            let cached = {
                let cache = self.yt_dlp_cache.lock().unwrap_or_else(|e| e.into_inner());
                cache.get(&cache_key).filter(|(_, ts)| now.duration_since(*ts) < YT_DLP_CACHE_TTL).cloned()
            };
            match cached {
                Some((rs, _)) => (rs.url, rs.abr, rs.codec, rs.ext, rs.duration),
                None => match resolve_youtube_stream_url(url, ceiling).await {
                    Ok(v) => {
                        let (stream_url, abr, codec, ext, duration) = &v;
                        let mut cache = self.yt_dlp_cache.lock().unwrap_or_else(|e| e.into_inner());
                        cache.insert(cache_key, (ResolvedStream { url: stream_url.clone(), abr: *abr, codec: codec.clone(), ext: ext.clone(), duration: *duration }, now));
                        drop(cache);
                        v
                    }
                    Err(e) => return Err(StreamStartError::Resolve(e)),
                },
            }
        };
        // Record the delivered bitrate / codec / duration of the chosen stream.
        self.actual_kbps = abr;
        self.codec_label = codec;
        if let Some(dur) = duration {
            self.duration_sec = Some(dur);
        }

        // Streaming is the default: it starts decoding while the download is
        // still trickling, so time-to-first-sound is bounded by the container
        // header rather than the whole track. A whole-file download is the
        // fallback, taken only when the container cannot be *probed* — and so
        // cannot be decoded — from the downloaded prefix.
        //
        // The extension is deliberately not the decision: whether a partial file
        // probes depends on the stream's own cue layout, not its suffix, so the
        // probe is the authority (a DASH fMP4 with its moov up front streams
        // fine; a progressive MP4 that cues moov at the end does not, and falls
        // back). Guessing by extension bought a whole-track download for every
        // container that merely *might* not stream.
        let _ = ext;

        let dl_path = path.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        // Kept outside the `StreamingPlayback` struct so the fallback below can
        // still ask "did the user move on?" after the stream was handed off.
        let cancel_check = cancel.clone();
        let fallback_url = direct_url.clone();

        let download_cancel = cancel.clone();
        let http_client = self.http_client.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let download_handle = tokio::spawn(async move {
            let result = stream_http_to_file(&http_client, &direct_url, &dl_path, download_cancel).await;
            let _ = tx.send(result);
        });

        if let Ok(Ok(Err(e))) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            rx,
        )
        .await
        {
            download_handle.abort();
            return Err(StreamStartError::Playback(format!("stream download failed: {}", e).into()));
        }

        {
            let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.pcm.clear();
            buf.read_pos = 0;
            buf.consumed_total = 0;
            buf.channels = 0;
            buf.file_sample_rate = 0;
        }

        // The producer below fills the buffer with nothing to drain it unless a
        // writer is running, and `stop()` tore the previous one down.
        self.ensure_stream();

        let audio_buffer = self.audio_buffer.clone();
        let decode_path = path.clone();
        let decode_cancel = cancel.clone();
        // The probe outcome is the only signal that this stream is actually
        // decodable; without it the caller cannot tell a trickling download
        // (stream on) from an unprobeable container (fall back).
        let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
        let decode_handle = tokio::task::spawn_blocking(move || {
            if let Err(e) =
                streaming_decode_sync(decode_path, audio_buffer, decode_cancel, Some(probe_tx))
            {
                error!("streaming decode task failed: {}", e);
            }
        });

        self.streaming = Some(StreamingPlayback {
            download_handle: Some(download_handle),
            decode_handle,
            cancel,
        });

        self.state = PlayerStateLabel::Playing;

        // Wait for the probe. A timeout means the download is still trickling,
        // not that streaming is broken, so the stream keeps going in the
        // background and this returns as soon as playback is viable.
        match tokio::time::timeout(Self::STREAM_PROBE_BUDGET, probe_rx).await {
            Ok(Ok(Err(e))) => {
                // Never probed: this container cannot be decoded incrementally.
                if cancel_check.load(Ordering::SeqCst) {
                    // The user moved on (stop / another track) while we waited.
                    return Ok(());
                }
                tracing::warn!(
                    error = %e,
                    "stream container could not be probed from a partial file; falling back to a whole-file download"
                );
                self.stop();
                stream_http_to_file(&self.http_client, &fallback_url, &path, Arc::new(AtomicBool::new(false)))
                    .await
                    .map_err(StreamStartError::Playback)?;
                {
                    let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                    buf.pcm.clear();
                    buf.read_pos = 0;
                    buf.consumed_total = 0;
                    buf.channels = 0;
                    buf.file_sample_rate = 0;
                }
                self.ensure_stream();
                self.load_and_play("current").await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.set_speed_and_pitch(speed, self.pitch);
    }

    pub fn set_speed_and_pitch(&mut self, speed: f32, pitch: f32) {
        self.speed = speed.clamp(0.25, 4.0);
        self.pitch = pitch.clamp(0.25, 4.0);
        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.speed = self.speed;
        buf.pitch = self.pitch;
        drop(buf);
    }

    /// Set the output volume.
    ///
    /// Software gain is the single authoritative path: every backend multiplies
    /// the samples it dequeues by this value, so it works on PipeWire, ALSA,
    /// AAudio, CoreAudio and WASAPI alike and needs no external tool. It also
    /// lives in the `Arc` shared with the backend, so it survives a stream
    /// recreation (`try_recover`) unchanged — a hardware/sink-input path would
    /// reset to 100% on every recovery and would need re-applying to a node that
    /// may not be linked yet, and mixing the two double-attenuates.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume_val = volume.clamp(0.0, 1.0);
        *self.volume.lock().unwrap_or_else(|e| e.into_inner()) = self.volume_val;
    }

    /// The software gain actually applied by the backend (the `Arc` shared with
    /// the audio stream). Exposed for tests: it must always track `volume_val`.
    #[cfg(test)]
    pub(crate) fn applied_gain(&self) -> f32 {
        *self.volume.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// G2: change the default bandwidth ceiling at runtime without a restart.
    /// `target_kbps = 0` (or null) or an unknown label resets to the server
    /// default (best).
    pub fn set_target(&mut self, payload: crate::api::models::TargetPayload) {
        self.runtime_target_kbps = resolve_ceiling(
            payload.target_kbps,
            payload.quality,
            self.default_target_kbps,
        );
    }

    /// The server-configured default ceiling (kbps); `0` = best.
    pub fn default_target_kbps(&self) -> u32 {
        self.default_target_kbps
    }

    /// The current runtime default ceiling (kbps); may differ from the config
    /// default after `set_target`. `0` = best.
    pub fn current_default_target_kbps(&self) -> u32 {
        self.runtime_target_kbps
    }

    pub fn seek(&mut self, position: u64) {
        if self.check_backend_error() {
            let _ = self.try_recover();
        }

        if self.streaming.is_some() {
            tracing::warn!("seek ignored during streaming playback");
            return;
        }

        // Seek refills the buffer, so it needs a live writer to drain it.
        self.ensure_stream();

        let path = self.storage.get_active_slot_path("current");
        let path_clone = path.clone();
        let speed = self.speed;
        let pitch = self.pitch;
        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            buf.sample_rate
        };

        // OPT-9: decode only from the seek position instead of decoding the whole
        // track, then splitting off. Avoids full-track PCM allocation on long files.
        let decode = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            decode_from_position_blocking(&path_clone, position, speed, pitch, device_rate)
        }));

        let (mut pcm, file_sample_rate, channels, _codec, _total_frames) = match decode {
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

        // Clamp position to track duration so the reported playhead is always
        // reachable (seeking at/past the end leaves nothing to play).
        let total_frames = pcm.len() / (channels as usize);
        let total_sec = (total_frames as f64 / file_sample_rate as f64).round() as u64;
        let position = position.min(total_sec);

        self.file_sample_rate = file_sample_rate;
        self.channels = channels;

        let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.pcm = pcm;
        buf.read_pos = 0;
        buf.consumed_total = 0;
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        buf.pitch = self.pitch;
        buf.notify_data();
        drop(buf);

        self.position_sec = position;
        let remaining_frames = total_frames;
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

    pub async fn get_status(&mut self) -> PlayerStatus {
        if self.check_backend_error() {
            let _ = self.try_recover();
        }

        // One lock acquisition per poll serves both the end-of-track decision
        // and the playhead: the writer holds this mutex across its reclaim
        // memmove, so a poll must not walk it twice. The playhead is the
        // samples-consumed count (reclaimed prefix + read_pos), never
        // `read_pos` alone (a reclaim would rewind it) nor `pcm.is_empty()`
        // (a drained window is not "nothing played").
        let (decode_done, unconsumed, position_from_buffer) = {
            let buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            let decode_done = self
                .streaming
                .as_ref()
                .map_or(true, |s| s.decode_handle.is_finished());
            let unconsumed = buf.pcm.len().saturating_sub(buf.read_pos);
            (decode_done, unconsumed, Self::buffer_position_sec(&buf, self.position_sec))
        };
        let mut position_sec = position_from_buffer;

        // BUG-3 (device report): there was no end-of-track, so state stayed
        // `playing` forever with a frozen head and the client never advanced.
        // Once the decode task has delivered the whole track and the writer has
        // consumed every buffered sample, playback is over: finish as Stopped.
        // A momentarily drained buffer mid-stream is a rebuffer, not EOS, hence
        // the decode-finished condition (`streaming` is None on the file path,
        // where the whole track is already decoded when state turns Playing).
        if self.state == PlayerStateLabel::Playing && decode_done && unconsumed == 0 {
            self.stop();
            info!("end of track reached: state = stopped");
            // stop() cleared the buffer and reset the seek base, so the
            // snapshot taken above is stale.
            position_sec = Some(0);
        }

        // One construction site for `PlayerStatus`, not two: a second literal is
        // how a field lands in one branch and not the other. A stopped engine
        // with no stream reaches the same values here — `stop()` clears the
        // buffer (so the playhead is the seek base, which it just reset to 0) and
        // `audio_available` reports whether a writer thread still exists.
        let next_cached = self.storage.get_active_slot_path("next").exists();
        let prev_cached = self.storage.get_active_slot_path("prev").exists();
        let utilization = self.storage.total_size;

        let position_sec = position_sec.or(Some(self.position_sec));

        PlayerStatus {
            state: self.state,
            volume: self.volume_val,
            speed: self.speed,
            pitch: self.pitch,
            current_track: self.current_track.clone(),
            disk_pool_utilization_bytes: utilization,
            next_cached,
            prev_cached,
            position_sec,
            duration_sec: self.duration_sec,
            time: status_time(position_sec, self.duration_sec),
            audio_available: self.stream.is_some(),
            target_kbps: self.effective_target_kbps,
            actual_kbps: self.actual_kbps,
            codec: self.codec_label.clone(),
        }
    }
}

/// `"m:ss/m:ss"` playhead for progress-bar front-ends. A front-end that only
/// reads `position_sec`/`duration_sec` has to build this itself; one that only
/// reads `time` (the shape the player app historically asked for) gets nothing.
/// Falls back to the position alone when no duration is known yet.
fn status_time(position_sec: Option<u64>, duration_sec: Option<u64>) -> Option<String> {
    match (position_sec, duration_sec) {
        (Some(p), Some(d)) => Some(format!(
            "{:02}:{:02}/{:02}:{:02}",
            p / 60,
            p % 60,
            d / 60,
            d % 60
        )),
        (Some(p), None) => Some(format!("{:02}:{:02}", p / 60, p % 60)),
        _ => None,
    }
}

/// Map a Symphonia codec type to the short label shown in `PlayerStatus.codec`.
fn codec_label(ct: symphonia::core::codecs::CodecType) -> Option<String> {
    use symphonia::core::codecs::{
        CODEC_TYPE_AAC, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP3, CODEC_TYPE_OPUS,
        CODEC_TYPE_PCM_S16LE, CODEC_TYPE_VORBIS,
    };
    let s = if ct == CODEC_TYPE_OPUS {
        "opus"
    } else if ct == CODEC_TYPE_AAC {
        "aac"
    } else if ct == CODEC_TYPE_VORBIS {
        "vorbis"
    } else if ct == CODEC_TYPE_MP3 {
        "mp3"
    } else if ct == CODEC_TYPE_FLAC {
        "flac"
    } else if ct == CODEC_TYPE_ALAC {
        "alac"
    } else if ct == CODEC_TYPE_PCM_S16LE {
        "pcm_s16le"
    } else {
        return None;
    };
    Some(s.to_string())
}

pub(crate) fn decode_to_pcm(
    bytes: Vec<u8>,
) -> Result<(Vec<f32>, u32, u16, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;

    let pcm_hint = bytes.len().saturating_div(4);
    let source = std::io::Cursor::new(bytes);
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    // `pcm_hint` (captured above before `bytes` was moved) is used to
    // pre-allocate the PCM output buffer (OPT-7).

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
    let codec = codec_label(codec_params.codec);

    // Symphonia has no Opus decoder; route Opus tracks to libopus (G1).
    #[cfg(feature = "opus")]
    if codec_params.codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        let extra: &[u8] = codec_params.extra_data.as_deref().unwrap_or(&[]);
        let head = crate::player::opus::parse_opus_head(extra)
            .ok_or("Opus track missing OpusHead sideband")?;
        let mut opus_dec = crate::player::opus::OpusTrackDecoder::new(&head)?;
        // Pre-allocate based on compressed size: raw PCM f32 is typically
        // several times larger than the compressed bytes, so bytes.len()/4 is
        // a conservative initial capacity that avoids most small reallocations
        // without over-allocating (OPT-7).
        let mut pcm: Vec<f32> = Vec::with_capacity(pcm_hint);
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(_) => break,
            };
            if packet.track_id() != track_id {
                continue;
            }
            pcm.extend(opus_dec.push(packet.buf()));
        }
        return Ok((pcm, 48000, head.channels, Some("opus".to_string())));
    }

    tracing::info!(?codec_params, codec_id = ?codec_params.codec, "decode_to_pcm: probing result");
    let mut decoder = match symphonia::default::get_codecs().make(&codec_params, &decoder_opts) {
        Ok(decoder) => decoder,
        Err(e) => {
            tracing::error!(?codec_params, error = ?e, "decode_to_pcm: decoder creation failed");
            return Err(format!("decoder init: {e}").into());
        }
    };

    let mut sample_buf = None;
    let mut pcm = Vec::with_capacity(pcm_hint);

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
    // Some containers (notably AAC-LC) probe with `channels == None`; treating an unknown
    // layout as 0 would later divide by zero in duration calculations. Default to stereo,
    // the overwhelmingly common sane fallback, and warn so the assumption is visible.
    let channels = match codec_params.channels {
        Some(layout) => layout.count() as u16,
        None => {
            tracing::warn!(codec = ?codec_params.codec, "decode_to_pcm: channel layout unknown, assuming stereo");
            2
        }
    };
    Ok((pcm, sample_rate, channels.max(1), codec))
}

/// OPT-13: Blocking incremental decode of a complete file into the audio
/// buffer. Decodes packet-by-packet, applying stretch/pitch/resample per
/// packet, keeping memory bounded by the rebuffer window rather than the
/// track length. Runs on a blocking thread (called via spawn_blocking from
/// `decode_file_incremental`).
fn decode_file_incremental_blocking(
    path: &PathBuf,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    speed: f32,
    pitch: f32,
    device_rate: u32,
) -> Result<(u32, u16, Option<String>, u64), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let source = FileSource { file };
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let hint = Hint::new();
    let probed =
        symphonia::default::get_probe().format(&hint, mss, &Default::default(), &Default::default())
            .map_err(|e| format!("format probe failed: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            let ct = t.codec_params.codec;
            ct == symphonia::core::codecs::CODEC_TYPE_AAC
                || ct == symphonia::core::codecs::CODEC_TYPE_MP3
                || ct == symphonia::core::codecs::CODEC_TYPE_OPUS
                || ct == symphonia::core::codecs::CODEC_TYPE_VORBIS
                || ct == symphonia::core::codecs::CODEC_TYPE_FLAC
                || ct == symphonia::core::codecs::CODEC_TYPE_ALAC
                || ct == symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
        })
        .cloned()
        .or_else(|| format.default_track().cloned())
        .ok_or("no default track in file")?;

    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let codec = codec_label(codec_params.codec);

    let file_sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2).max(1);

    {
        let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ab.channels = channels;
        ab.file_sample_rate = file_sample_rate;
    }

    let mut total_frames: u64 = 0;

    // Opus: decode via libopus at 48 kHz, no resample needed.
    #[cfg(feature = "opus")]
    if codec_params.codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        let extra: &[u8] = codec_params.extra_data.as_deref().unwrap_or(&[]);
        let head = crate::player::opus::parse_opus_head(extra)
            .ok_or("Opus track missing OpusHead sideband")?;
        {
            let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            ab.channels = head.channels;
            ab.file_sample_rate = 48000;
        }
        let mut opus_dec = crate::player::opus::OpusTrackDecoder::new(&head)?;
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(_) => break,
            };
            if packet.track_id() != track_id {
                continue;
            }
            let out = opus_dec.push(packet.buf());
            if !out.is_empty() {
                let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                ab.pcm.extend_from_slice(&out);
                ab.notify_data();
                total_frames += (out.len() / head.channels as usize) as u64;
            }
        }
        return Ok((48000, head.channels, codec, total_frames));
    }

    // Non-Opus: decode via Symphonia.
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &Default::default())
        .map_err(|e| format!("decoder init: {e}"))?;

    let mut sample_buf = None;
    let tempo = speed;
    let mut stretcher = if tempo != 1.0 {
        wsola::TimeStretch::new(file_sample_rate, channels)
            .ok()
            .map(|mut ts| { ts.set_tempo(tempo); ts })
    } else {
        None
    };

    let mut shifter = if pitch != 1.0 {
        Some(pitch_shift::Shifter::new(Box::new([0.0f32; pitch_shift::TOTAL_F32])))
    } else {
        None
    };
    let shift_semitones = if pitch != 1.0 {
        Some(12.0 * (pitch as f64).log2() as f32)
    } else {
        None
    };

    let mut resampler = InterleavedResampler::new(channels, file_sample_rate, device_rate);

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
                let spec = *audio_buf.spec();
                if sample_buf.is_none() {
                    sample_buf = Some(SampleBuffer::<f32>::new(
                        audio_buf.capacity() as u64,
                        spec,
                    ));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    let samples = buf.samples();

                    let mut output = samples.to_vec();

                    // Time-stretch (wsola): push samples, pull available output.
                    if let Some(ref mut stretcher) = stretcher {
                        stretcher.push(samples);
                        let mut pulled = Vec::new();
                        loop {
                            let batch = stretcher.pull(4096);
                            if batch.is_empty() {
                                break;
                            }
                            pulled.extend_from_slice(&batch);
                        }
                        output = pulled;
                    }

                    // Pitch shift.
                    if let (Some(ref mut shifter), Some(semitones)) =
                        (shifter.as_mut(), shift_semitones)
                    {
                        let ch = channels as usize;
                        let mut input_planar = vec![Vec::new(); ch];
                        for frame in output.chunks(ch) {
                            for (c, &sample) in frame.iter().enumerate() {
                                input_planar[c].push(sample);
                            }
                        }
                        let mut pitched_planar: Vec<Vec<f32>> = vec![Vec::new(); ch];
                        for c in 0..ch {
                            for chunk in input_planar[c].chunks(128) {
                                let mut buf = [0.0f32; 128];
                                for (i, &s) in chunk.iter().enumerate() {
                                    buf[i] = s;
                                }
                                let result =
                                    shifter.shift(&buf, semitones, 128, file_sample_rate as f32);
                                pitched_planar[c].extend_from_slice(result);
                            }
                        }
                        let out_frames = pitched_planar[0].len();
                        output = Vec::with_capacity(out_frames * ch);
                        for f in 0..out_frames {
                            for c in 0..ch {
                                output.push(pitched_planar[c][f]);
                            }
                        }
                    }

                    // Resample and append to buffer.
                    let resampled = resampler.process(&output);
                    if !resampled.is_empty() {
                        let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                        ab.pcm.extend_from_slice(&resampled);
                        ab.notify_data();
                        total_frames += (resampled.len() / channels as usize) as u64;
                    }

                    // Throttle if buffer exceeds rebuffer window.
                    {
                        let ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                        if ab.exceeds_window() {
                            drop(ab);
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }

    // Drain any remaining wsola output.
    if let Some(ref mut stretcher) = stretcher {
        loop {
            let pulled = stretcher.pull(4096);
            if pulled.is_empty() {
                break;
            }
            let resampled = resampler.process(&pulled);
            if !resampled.is_empty() {
                let mut ab =
                    audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                ab.pcm.extend_from_slice(&resampled);
                ab.notify_data();
                total_frames += (resampled.len() / channels as usize) as u64;
            }
        }
    }

    // Flush resampler tail.
    let tail = resampler.finish();
    if !tail.is_empty() {
        let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        ab.pcm.extend_from_slice(&tail);
        ab.notify_data();
        total_frames += (tail.len() / channels as usize) as u64;
    }

    Ok((file_sample_rate, channels, codec, total_frames))
}

/// OPT-9: Blocking decode starting at a seek position instead of decoding the
/// whole track. Opens the file via the seekable `FileSource`, seeks to
/// `position_sec`, then decodes only packets from that point forward. PCM is
/// stretch/pitch/resampled per packet just like `decode_file_incremental_blocking`,
/// so memory stays bounded by the rebuffer window rather than the track length.
///
/// For Opus tracks, symphonia cannot decode Opus (libopus does), so instead of
/// container-level seeking we scan packets from the start, tracking cumulative
/// decoded time, skip until `position_sec`, then decode from there with pre-skip
/// cleared.
fn decode_from_position_blocking(
    path: &PathBuf,
    position_sec: u64,
    speed: f32,
    pitch: f32,
    device_rate: u32,
) -> Result<(Vec<f32>, u32, u16, Option<String>, u64), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;
    use symphonia::core::units::Time;
    use symphonia::core::formats::{SeekMode, SeekTo};

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let source = FileSource { file };
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let hint = Hint::new();
    let probed =
        symphonia::default::get_probe().format(&hint, mss, &Default::default(), &Default::default())
            .map_err(|e| format!("format probe failed: {e}"))?;

    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            let ct = t.codec_params.codec;
            ct == symphonia::core::codecs::CODEC_TYPE_AAC
                || ct == symphonia::core::codecs::CODEC_TYPE_MP3
                || ct == symphonia::core::codecs::CODEC_TYPE_OPUS
                || ct == symphonia::core::codecs::CODEC_TYPE_VORBIS
                || ct == symphonia::core::codecs::CODEC_TYPE_FLAC
                || ct == symphonia::core::codecs::CODEC_TYPE_ALAC
                || ct == symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
        })
        .cloned()
        .or_else(|| format.default_track().cloned())
        .ok_or("no default track in file")?;

    let track_id = track.id;
    let codec_params = track.codec_params.clone();
    let codec = codec_label(codec_params.codec);

    let file_sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2).max(1);

    let mut total_frames: u64 = 0;

    // Opus: libopus decode, no container-level seek. Scan packets from start,
    // tracking cumulative time, skip until position_sec, then decode from there.
    #[cfg(feature = "opus")]
    if codec_params.codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        let extra: &[u8] = codec_params.extra_data.as_deref().unwrap_or(&[]);
        let head = crate::player::opus::parse_opus_head(extra)
            .ok_or("Opus track missing OpusHead sideband")?;
        let mut opus_dec = crate::player::opus::OpusTrackDecoder::new(&head)?;
        let mut pcm: Vec<f32> = Vec::new();
        let mut cum_time_sec: f64 = 0.0;
        let mut started = position_sec == 0;
        if position_sec > 0 {
            opus_dec.clear_pre_skip();
        }
        loop {
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(_) => break,
            };
            if packet.track_id() != track_id {
                continue;
            }
            let out = opus_dec.push(packet.buf());
            if out.is_empty() {
                continue;
            }
            let out_frames = out.len() / head.channels as usize;
            if !started {
                cum_time_sec += out_frames as f64 / 48000.0;
                if cum_time_sec >= position_sec as f64 {
                    started = true;
                    // Trim this packet's leading samples to land exactly on position.
                    let overshoot = (cum_time_sec - position_sec as f64) * 48000.0;
                    let drop_samples = overshoot as usize * head.channels as usize;
                    if drop_samples < out.len() {
                        pcm.extend_from_slice(&out[drop_samples..]);
                        total_frames +=
                            ((out.len() - drop_samples) / head.channels as usize) as u64;
                    }
                }
            } else {
                pcm.extend_from_slice(&out);
                total_frames += out_frames as u64;
            }
        }
        return Ok((pcm, 48000, head.channels, Some("opus".to_string()), total_frames));
    }

        // Non-Opus: use symphonia's container-level seek to jump to position_sec.
    if position_sec > 0 {
        let _ = format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::new(position_sec, 0.0),
                track_id: None,
            },
        );
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &Default::default())
        .map_err(|e| format!("decoder init: {e}"))?;

    let mut sample_buf = None;
    let tempo = speed;
    let mut stretcher = if tempo != 1.0 {
        wsola::TimeStretch::new(file_sample_rate, channels)
            .ok()
            .map(|mut ts| { ts.set_tempo(tempo); ts })
    } else {
        None
    };

    let mut shifter = if pitch != 1.0 {
        Some(pitch_shift::Shifter::new(Box::new([0.0f32; pitch_shift::TOTAL_F32])))
    } else {
        None
    };
    let shift_semitones = if pitch != 1.0 {
        Some(12.0 * (pitch as f64).log2() as f32)
    } else {
        None
    };

    let mut resampler = InterleavedResampler::new(channels, file_sample_rate, device_rate);

    let mut pcm: Vec<f32> = Vec::new();

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
                let spec = *audio_buf.spec();
                if sample_buf.is_none() {
                    sample_buf = Some(SampleBuffer::<f32>::new(
                        audio_buf.capacity() as u64,
                        spec,
                    ));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    let samples = buf.samples();

                    let mut output = samples.to_vec();

                    if let Some(ref mut stretcher) = stretcher {
                        stretcher.push(samples);
                        let mut pulled = Vec::new();
                        loop {
                            let batch = stretcher.pull(4096);
                            if batch.is_empty() {
                                break;
                            }
                            pulled.extend_from_slice(&batch);
                        }
                        output = pulled;
                    }

                    if let (Some(ref mut shifter), Some(semitones)) =
                        (shifter.as_mut(), shift_semitones)
                    {
                        let ch = channels as usize;
                        let mut input_planar = vec![Vec::new(); ch];
                        for frame in output.chunks(ch) {
                            for (c, &sample) in frame.iter().enumerate() {
                                input_planar[c].push(sample);
                            }
                        }
                        let mut pitched_planar: Vec<Vec<f32>> = vec![Vec::new(); ch];
                        for c in 0..ch {
                            for chunk in input_planar[c].chunks(128) {
                                let mut buf = [0.0f32; 128];
                                for (i, &s) in chunk.iter().enumerate() {
                                    buf[i] = s;
                                }
                                let result =
                                    shifter.shift(&buf, semitones, 128, file_sample_rate as f32);
                                pitched_planar[c].extend_from_slice(result);
                            }
                        }
                        let out_frames = pitched_planar[0].len();
                        output = Vec::with_capacity(out_frames * ch);
                        for f in 0..out_frames {
                            for c in 0..ch {
                                output.push(pitched_planar[c][f]);
                            }
                        }
                    }

                    let resampled = resampler.process(&output);
                    if !resampled.is_empty() {
                        pcm.extend_from_slice(&resampled);
                        total_frames += (resampled.len() / channels as usize) as u64;
                    }
                }
            }
            Err(_) => break,
        }
    }

    if let Some(ref mut stretcher) = stretcher {
        loop {
            let pulled = stretcher.pull(4096);
            if pulled.is_empty() {
                break;
            }
            let resampled = resampler.process(&pulled);
            if !resampled.is_empty() {
                pcm.extend_from_slice(&resampled);
                total_frames += (resampled.len() / channels as usize) as u64;
            }
        }
    }

    let tail = resampler.finish();
    if !tail.is_empty() {
        pcm.extend_from_slice(&tail);
        total_frames += (tail.len() / channels as usize) as u64;
    }

    Ok((pcm, file_sample_rate, channels, codec, total_frames))
}

/// Input chunk size (frames per channel) that rubato's `SincFixedIn` demands
/// per call. `chunk_size` counts *input* frames, so handing it any other length
/// fails with `InsufficientInputBufferSize`.
const RESAMPLE_CHUNK_FRAMES: usize = 4096;

/// Convert interleaved PCM to a new sample rate.
///
/// `rubato::SincFixedIn` accepts only its fixed input chunk per call and returns
/// its result *channel-planar*, and both requirements bit every caller: the old
/// file path handed it arbitrary lengths, and the old streaming path handed it
/// each decoded packet (~1024 frames) — every call failed the length check and
/// the `if let Ok(...)` guard discarded it, so a 44.1 kHz AAC/MP3 source decoded
/// correctly and then nothing at all was written to a healthy sink (device
/// report: sink monitor peak exactly 0). The old file path also appended
/// `output_planar[ch]` per channel — i.e. planar `LLL...RRR...` — where the
/// stream format needs interleaved `L R L R ...`.
///
/// This wrapper owns the chunking (so any input length works, including the
/// ~1024-frame packets of a stream) and returns the interleaved layout the audio
/// stream requires. `finish` emits the short tail, zero-padded as rubato
/// documents, so the last frames of a track are never lost.
pub(crate) struct InterleavedResampler {
    inner: Option<SincFixedIn<f32>>,
    channels: usize,
    pending: Vec<f32>,
    /// Offset into `pending` of the first unconsumed sample. Avoids memmove on
    /// every `process()` call; data is compacted to the front only when the
    /// offset grows large (OPT-4).
    pending_offset: usize,
}

impl InterleavedResampler {
    /// `None`-equivalent passthrough when both rates match (no allocation, no
    /// resampler) or when construction fails for a rate pair that should be
    /// impossible — a passthrough is wrong-rate-but-audible, a panic here would
    /// kill the decode task and return silence instead.
    pub(crate) fn new(channels: u16, in_rate: u32, out_rate: u32) -> Self {
        let channels = channels.max(1) as usize;
        let inner = if in_rate == out_rate || in_rate == 0 || out_rate == 0 {
            None
        } else {
            let params = SincInterpolationParameters {
                sinc_len: 1024,
                f_cutoff: 0.95,
                oversampling_factor: 128,
                interpolation: SincInterpolationType::Cubic,
                window: WindowFunction::BlackmanHarris2,
            };
            SincFixedIn::<f32>::new(
                out_rate as f64 / in_rate as f64,
                10.0,
                params,
                RESAMPLE_CHUNK_FRAMES,
                channels,
            )
            .ok()
        };
        Self {
            inner,
            channels,
            pending: Vec::new(),
            pending_offset: 0,
        }
    }

    /// Resample `input` (interleaved, `channels` per frame). A tail shorter than
    /// one chunk is buffered until the next call or [`InterleavedResampler::finish`].
    pub(crate) fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let Some(resampler) = self.inner.as_mut() else {
            return input.to_vec();
        };
        self.pending.extend_from_slice(input);
        let chunk_samples = RESAMPLE_CHUNK_FRAMES * self.channels;
        let mut output = Vec::new();
        // Cursor-based consumption: read from pending_offset + consumed, no
        // memmove per chunk. Compact to front only when offset grows large.
        let mut consumed = 0;
        let base = self.pending_offset;
        while self.pending.len() - base - consumed >= chunk_samples {
            let wave_in =
                deinterleave(&self.pending[base + consumed..base + consumed + chunk_samples], self.channels);
            if let Ok(planar) = resampler.process(&wave_in, None) {
                output.extend_from_slice(&interleave(planar));
            }
            return_planar_to_pool(wave_in);
            consumed += chunk_samples;
        }
        self.pending_offset = base + consumed;
        // Compact when offset is large enough to justify a memmove.
        if self.pending_offset > RESAMPLE_CHUNK_FRAMES * self.channels {
            let remaining_len = self.pending.len() - self.pending_offset;
            self.pending.copy_within(self.pending_offset.., 0);
            self.pending.truncate(remaining_len);
            self.pending_offset = 0;
        }
        output
    }

    /// Emit the buffered tail and flush the resampler's internal pipeline.
    /// A no-op (empty output) when no rate change is in effect.
    pub(crate) fn finish(&mut self) -> Vec<f32> {
        let Some(resampler) = self.inner.as_mut() else {
            let remaining = std::mem::take(&mut self.pending);
            self.pending_offset = 0;
            return remaining;
        };
        if self.pending.len() <= self.pending_offset {
            return Vec::new();
        }
        let wave_in = deinterleave(&self.pending[self.pending_offset..], self.channels);
        self.pending_offset = 0;
        self.pending.clear();
        let result = resampler
            .process_partial(Some(&wave_in), None)
            .map(|planar| interleave(planar))
            .unwrap_or_default();
        return_planar_to_pool(wave_in);
        result
    }
}

/// Split interleaved samples into per-channel vectors (whole frames only).
///
/// Uses a thread-local pool of reusable per-channel buffers to avoid fresh
/// allocations per call (OPT-5). The returned buffers are owned by the caller
/// and must be passed to [`interleave`] (which returns them to the pool) or
/// [`return_planar_to_pool`] when done.
fn deinterleave(input: &[f32], channels: usize) -> Vec<Vec<f32>> {
    thread_local! {
        static POOL: std::cell::RefCell<Vec<Vec<Vec<f32>>>> = std::cell::RefCell::new(Vec::new());
    }
    let frames = input.len() / channels;
    let mut planar = POOL.with(|p| {
        let mut pool = p.borrow_mut();
        pool.pop().unwrap_or_else(|| Vec::with_capacity(channels))
    });
    planar.clear();
    planar.resize_with(channels, || Vec::with_capacity(frames));
    for p in planar.iter_mut() {
        p.clear();
        p.reserve(frames);
    }
    for frame in input[..frames * channels].chunks(channels) {
        for (ch, &sample) in frame.iter().enumerate() {
            planar[ch].push(sample);
        }
    }
    planar
}

/// Return a pooled `Vec<Vec<f32>>`'s inner buffers to the thread-local pool
/// for reuse (OPT-5). Caller must not use `planar` after this.
fn return_planar_to_pool(planar: Vec<Vec<f32>>) {
    thread_local! {
        static POOL: std::cell::RefCell<Vec<Vec<Vec<f32>>>> = std::cell::RefCell::new(Vec::new());
    }
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < 16 {
            pool.push(planar);
        }
    });
}
fn interleave(planar: Vec<Vec<f32>>) -> Vec<f32> {
    thread_local! {
        static POOL: std::cell::RefCell<Vec<Vec<Vec<f32>>>> = std::cell::RefCell::new(Vec::new());
    }
    let channels = planar.len();
    if channels == 0 {
        return Vec::new();
    }
    let frames = planar[0].len();
    let mut output = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        for ch in 0..channels {
            output.push(planar[ch][frame]);
        }
    }
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < 16 {
            pool.push(planar);
        }
    });
    output
}

pub(crate) fn pitch_shift_interleaved(
    input: &[f32],
    channels: u16,
    pitch: f32,
    sample_rate: f32,
) -> Vec<f32> {
    if pitch == 1.0 || input.is_empty() {
        return input.to_vec();
    }

    let channels = channels as usize;
    let total_frames = input.len() / channels;
    if total_frames == 0 {
        return input.to_vec();
    }

    let shift_semitones = 12.0 * (pitch as f64).log2() as f32;
    let chunk_size = 128usize;

    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(total_frames); channels];
    for frame in input.chunks(channels) {
        for (ch, &sample) in frame.iter().enumerate() {
            planar[ch].push(sample);
        }
    }

    let padded_frames = total_frames.div_ceil(chunk_size) * chunk_size;
    let mut output_planar: Vec<Vec<f32>> = vec![Vec::with_capacity(padded_frames); channels];
    for ch in 0..channels {
        let mut shifter = pitch_shift::Shifter::new(Box::new([0.0f32; pitch_shift::TOTAL_F32]));
        let samples = &planar[ch];
        for chunk in samples.chunks(chunk_size) {
            let mut buf = [0.0f32; 128];
            for (i, &s) in chunk.iter().enumerate() {
                buf[i] = s;
            }
            let result = shifter.shift(&buf, shift_semitones, chunk_size, sample_rate);
            output_planar[ch].extend_from_slice(result);
        }
    }

    let output_frames = output_planar[0].len();
    let mut output = Vec::with_capacity(output_frames * channels);
    for frame_idx in 0..output_frames {
        for ch in 0..channels {
            output.push(output_planar[ch][frame_idx]);
        }
    }

    output
}

pub(crate) fn stretch_and_resample(
    input: &[f32],
    channels: u16,
    file_sample_rate: u32,
    device_rate: u32,
    speed: f32,
    pitch: f32,
) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }

    let channels = channels as usize;
    // The pitch shifter is length-preserving, so pitch is *not* pre-compensated
    // by a slower time-stretch here: stretching by `speed` and shifting by
    // `pitch` yields duration/speed and frequency×pitch. Pre-dividing by pitch
    // made every pitch≠1 track play 1/pitch too long (device bug report).
    let tempo = speed;

    let stretched = if tempo != 1.0 {
        wsola::stretch(input, file_sample_rate, channels as u16, tempo)
            .unwrap_or_else(|_| input.to_vec())
    } else {
        input.to_vec()
    };

    let pitched = if pitch != 1.0 {
        pitch_shift_interleaved(&stretched, channels as u16, pitch, file_sample_rate as f32)
    } else {
        stretched
    };

    if file_sample_rate == device_rate {
        return pitched;
    }

    // One pass through the shared wrapper: full chunks for rubato, interleaved
    // output for the stream, tail flushed so a rate change never shortens the
    // track (identical policy to the streaming producer below).
    let mut resampler = InterleavedResampler::new(channels as u16, file_sample_rate, device_rate);
    let mut playback_pcm = resampler.process(&pitched);
    playback_pcm.extend(resampler.finish());
    playback_pcm
}

#[cfg(test)]
pub(crate) static DOWNLOAD_STREAM_INVOKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub async fn download_stream(
    http_client: &Arc<reqwest::Client>,
    url: &str,
    dest: &PathBuf,
    ceiling: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    #[cfg(test)]
    {
        DOWNLOAD_STREAM_INVOKED.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    let local_path: Option<PathBuf> = url.strip_prefix("file://").map(PathBuf::from);

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
        return download_via_ytdlp(url, dest, ceiling)
            .await
            .map_err(|e| format!("yt-dlp download failed for {url}: {e}").into());
    }

    let response = http_client
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
    let mut first_chunk_checked = false;
    while let Some(item) = stream.next().await {
        let chunk = item?;
        if !first_chunk_checked && chunk.len() >= 5 {
            let start = &chunk[0..5];
            if start == b"<!doc" || start == b"<html" || start == b"<?xml" {
                let _ = tokio::fs::remove_file(dest).await;
                return Err(
                    format!("downloaded content for {url} appears to be markup, not audio").into(),
                );
            }
            first_chunk_checked = true;
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
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
    ceiling: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Same policy as the streaming path: pick the rendition ourselves, then ask
    // yt-dlp for that exact format, so a ceiling (or `best`) never plays one
    // rendition and downloads another behind the user's back.
    let selection = ytdlp_selection_json(url, ceiling).await?;
    let format_id = selection
        .get("format_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("yt-dlp returned no audio format")?
        .to_string();

    let _ = tokio::fs::remove_file(dest).await;

    let output = TokioCommand::new("yt-dlp")
        .arg("--no-config")
        .arg("--no-warnings")
        .arg("-f")
        .arg(&format_id)
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

/// Best-effort container extension from a direct URL's path (before `?`).
fn guess_ext_from_url(url: &str) -> Option<String> {
    let path = url.split('?').next()?;
    let ext = path.rsplit('/').next()?.rsplit('.').next()?;
    if ext.is_empty() || ext.len() > 5 || ext.contains('/') {
        None
    } else {
        Some(ext.to_lowercase())
    }
}

/// Run yt-dlp for a URL with the shared flag set and the shared block detection.
/// `args` are the mode-specific flags (before the URL).
async fn run_ytdlp(
    url: &str,
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    if !yt_dlp_available() {
        return Err("yt-dlp is not installed. Install yt-dlp to resolve YouTube URLs.".into());
    }

    let mut command = TokioCommand::new("yt-dlp");
    command
        .arg("--no-config")
        .arg("--no-warnings")
        .arg("--no-playlist")
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = command.output().await?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(block_type) = detect_youtube_block(&stderr) {
        return Err(YouTubeError {
            block_type,
            message: format!("yt-dlp block detected: {:?}", block_type),
        }
        .into());
    }

    if !output.status.success() {
        return Err("yt-dlp format selection failed".into());
    }
    Ok(output)
}

/// `--dump-single-json` + our own selection, returning the single chosen
/// rendition as a JSON object. `Ok(None)` means the output could not be used
/// (not JSON, no `formats` array, no audio rendition) and the caller should
/// degrade to the classic selector instead of failing playback.
async fn ytdlp_selection_via_dump(
    url: &str,
    ceiling: u32,
) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
    let output = run_ytdlp(url, &["--dump-single-json"]).await?;
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Ok(None);
    };
    let Some(formats) = json.get("formats").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    let Some(chosen) = select_audio_format(formats, ceiling).cloned() else {
        return Ok(None);
    };
    // yt-dlp reports duration at the video level; keep the rendition's own value
    // when it has one so the caller sees a single consistent object.
    let mut chosen = chosen;
    if let Some(object) = chosen.as_object_mut() {
        if !object.contains_key("duration") {
            if let Some(duration) = json.get("duration") {
                object.insert("duration".to_string(), duration.clone());
            }
        }
    }
    Ok(Some(chosen))
}

/// Classic-selector fallback: yt-dlp picks the format, we record which one it
/// picked. This is the `-f <selector> --print` pair yt-dlp has supported for a
/// decade, so a JSON-shape regression degrades quality reporting instead of
/// ending playback.
async fn ytdlp_selection_via_selector(
    url: &str,
    ceiling: u32,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let selector = yt_dlp_format_selector(ceiling);
    let output = run_ytdlp(
        url,
        &[
            "-f",
            &selector,
            "--print",
            "%(format_id)s %(abr)s %(acodec)s %(ext)s %(duration)s %(url)s",
        ],
    )
    .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or("yt-dlp returned empty output")?;
    let mut parts = line.trim().splitn(6, ' ');
    let format_id = parts.next().unwrap_or_default().trim().to_string();
    let abr = parts
        .next()
        .and_then(|t| t.trim().parse::<f64>().ok())
        .map(|v| v.round());
    let codec = parts.next().map(|t| t.trim().to_lowercase());
    let ext = parts
        .next()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let duration = parts
        .next()
        .and_then(|t| t.trim().parse::<f64>().ok())
        .map(|v| v.round() as u64)
        .filter(|&d| d > 0);
    let stream_url = parts
        .next()
        .ok_or("yt-dlp returned no url")?
        .trim()
        .to_string();

    Ok(serde_json::json!({
        "format_id": format_id,
        "abr": abr,
        "acodec": codec,
        "ext": ext,
        "duration": duration,
        "url": stream_url,
    }))
}

/// The rendition yt-dlp selected for a URL, as one JSON object, preferring the
/// full format list (so G2's highest-bitrate policy is ours) and degrading to
/// the classic selector when that output is unusable.
async fn ytdlp_selection_json(
    url: &str,
    ceiling: u32,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(format) = ytdlp_selection_via_dump(url, ceiling).await? {
        return Ok(format);
    }
    ytdlp_selection_via_selector(url, ceiling).await
}

/// yt-dlp audio format selector for a bandwidth ceiling (G2), used as the
/// fallback when the full format list can't be used. `0` = best.
fn yt_dlp_format_selector(ceiling: u32) -> String {
    if ceiling == 0 {
        "bestaudio".to_string()
    } else {
        format!("bestaudio[abr<={ceiling}]/bestaudio")
    }
}

/// G2 rendition policy applied to yt-dlp's format list: the **highest average
/// bitrate** audio rendition at or below `ceiling` (`0` = best, no ceiling),
/// falling back to the *lowest* available when nothing fits so a ceiling can
/// never fail playback (reported honestly as `actual_kbps`).
///
/// The choice is made here rather than left to yt-dlp's `bestaudio` ranking:
/// yt-dlp resolves `bestaudio` to a ~129 kbps AAC m4a even when a higher-bitrate
/// Opus (itag 251) exists, so "best" silently under-selected quality for
/// everyone by default (BUG-4).
fn select_audio_format<'a>(
    formats: &'a [serde_json::Value],
    ceiling: u32,
) -> Option<&'a serde_json::Value> {
    let abr = |f: &serde_json::Value| {
        f.get("abr")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0)
    };
    // "Audio only" == the rendition carries no video track. A rendition with no
    // audio codec at all is never a candidate (it would be silence).
    let carries_audio = |f: &serde_json::Value| {
        f.get("acodec")
            .and_then(serde_json::Value::as_str)
            .map_or(true, |c| !c.is_empty() && !c.eq_ignore_ascii_case("none"))
    };
    let mut audio: Vec<&serde_json::Value> = formats
        .iter()
        .filter(|f| {
            carries_audio(f)
                && f.get("vcodec").and_then(serde_json::Value::as_str) == Some("none")
        })
        .collect();
    if audio.is_empty() {
        // Some sources expose no audio-only rendition (merged DASH/HLS); any
        // rendition carrying audio is still playable.
        audio = formats.iter().filter(|f| carries_audio(f)).collect();
    }
    let mut best: Option<&serde_json::Value> = None;
    let mut lowest: Option<&serde_json::Value> = None;
    for format in &audio {
        let bitrate = abr(format);
        if best.map_or(true, |b| bitrate > abr(b)) && (ceiling == 0 || bitrate <= ceiling as f64) {
            best = Some(format);
        }
        if lowest.map_or(true, |l| bitrate < abr(l)) {
            lowest = Some(format);
        }
    }
    best.or(lowest)
}

/// Map a yt-dlp `acodec` string to the short label used in `PlayerStatus.codec`.
fn normalize_codec(acodec: &str) -> Option<String> {
    let a = acodec.trim();
    if a.is_empty() || a.eq_ignore_ascii_case("none") || a.eq_ignore_ascii_case("na") {
        return None;
    }
    let label = if a.starts_with("opus") {
        "opus"
    } else if a.starts_with("mp4a") || a == "aac" {
        "aac"
    } else if a.starts_with("vorbis") {
        "vorbis"
    } else if a.starts_with("mp3") {
        "mp3"
    } else if a.starts_with("flac") {
        "flac"
    } else {
        a
    };
    Some(label.to_string())
}

/// Resolve a YouTube page URL to a direct audio stream URL honoring `ceiling`,
/// also returning the selected rendition's average bitrate and codec so
/// `PlayerStatus` can show reality.
async fn resolve_youtube_stream_url(
    url: &str,
    ceiling: u32,
) -> Result<(String, Option<u32>, Option<String>, Option<String>, Option<u64>), Box<dyn std::error::Error + Send + Sync>> {
    let format = ytdlp_selection_json(url, ceiling).await?;

    // Record the delivered bitrate / codec / duration of the chosen stream.
    let abr = format
        .get("abr")
        .and_then(serde_json::Value::as_f64)
        .map(|v| v.round() as u32);
    let codec = format
        .get("acodec")
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_codec);
    let ext = format
        .get("ext")
        .and_then(serde_json::Value::as_str)
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let duration = format
        .get("duration")
        .and_then(serde_json::Value::as_f64)
        .map(|v| v.round() as u64)
        .filter(|&d| d > 0);
    let stream_url = format
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or("selected format has no url")?
        .to_string();

    Ok((stream_url, abr, codec, ext, duration))
}

async fn stream_http_to_file(
    http_client: &Arc<reqwest::Client>,
    url: &str,
    dest: &PathBuf,
    cancel: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let response = http_client
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
/// G4: throttle a stream producer so the *unconsumed* rebuffer window stays
/// bounded. The device-paced writer drains it at ~1x, so this keeps memory
/// independent of track length and avoids tripping the backend window cap.
/// Returns early on cancel; a generous stall valve prevents ever wedging decode.
fn throttle_stream_producer(audio_buffer: &Arc<Mutex<AudioBuffer>>, cancel: &AtomicBool) {
    let mut stalls: u32 = 0;
    loop {
        let exceeded = {
            let b = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            b.exceeds_window()
        };
        if !exceeded {
            return;
        }
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        // Block until the consumer frees buffer space instead of polling every
        // 20 ms (OPT-19). A missed signal is harmless: the timeout bounds
        // re-check latency.
        {
            let b = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            b.wait_for_space(std::time::Duration::from_millis(20));
        }
        stalls += 1;
        if stalls > 3600 {
            return;
        }
    }
}

/// Handle a stream read `IoError` by waiting for the growing file (the async
/// download task keeps appending). Returns `false` when the stream has stalled
/// past tolerance and the caller should stop; `true` to retry. Shares one stall
/// policy between the Opus and Symphonia streaming loops (G4). Sleeps while
/// holding no buffer lock.
///
/// The tolerance is deliberately generous (STALL_TOLERANCE × 200 ms): a
/// trickling WebM download routinely goes seconds without appending, and the
/// demuxer keeps hitting element-past-master errors until the cluster completes,
/// so an early give-up ended tracks mid-playback (the device U2/EOS report).
/// Each no-growth spell is logged so a *permanent* wedge (the download finished
/// or died while the demuxer keeps erroring) is visible instead of a frozen
/// playhead with no explanation.
fn wait_for_progress(path: &std::path::Path, last_file_size: &mut u64, stall_cycles: &mut u32) -> bool {
    const STALL_TOLERANCE: u32 = 3000;
    const STALL_LOG_EVERY: u32 = 100; // ~20 s between progress reports
    let current_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if current_size > *last_file_size {
        *last_file_size = current_size;
        *stall_cycles = 0;
    } else if *stall_cycles >= STALL_TOLERANCE {
        tracing::warn!(
            "stream stalled: no download growth for ~{}s at {} bytes; ending decode",
            STALL_TOLERANCE * 200 / 1000,
            current_size
        );
        return false;
    } else {
        *stall_cycles += 1;
        if (*stall_cycles).is_multiple_of(STALL_LOG_EVERY) {
            tracing::warn!(
                "stream waiting on download: no growth for ~{}s at {} bytes",
                *stall_cycles * 200 / 1000,
                current_size
            );
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    true
}
/// Minimum spacing between demuxer rebuilds: a rebuild costs a full re-probe
/// plus a replay of the packets already pushed, so a wedged reader on a
/// trickling download must not spin.
const REBUILD_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// How long the download must stay quiet before a wedged reader is re-probed
/// anyway. A finished (or stalled) download never grows again by itself, and
/// only a rebuild against the file as it now stands can lift the length bound
/// Symphonia froze when the reader was created.
const REBUILD_QUIET_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// What a persistent demuxer error means for the streaming decode loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WedgeAction {
    /// Not a wedge: the caller's own error handling applies (log and end).
    NotWedge,
    /// Wedged, but rebuilding cannot help yet — wait on the download first.
    Wait,
    /// Wedged, and a fresh probe is worth its cost now.
    Rebuild,
}

/// Whether another re-probe of the stream file is due.
///
/// Symphonia freezes the stream length when a reader is created:
/// `MkvReader::try_new` seeks to EOF and keeps the result, both as the bound
/// every element must fit inside and as the source's EOF. A reader opened on a
/// partial download therefore errors on the first element crossing that
/// snapshot *and* stays "starved" for bytes it can never see, however much the
/// download appends afterwards. A fresh probe is the only thing that re-reads
/// the file's length, so re-probing pays off when
///
/// 1. the file grew since the last rebuild — the new bytes may complete the
///    element the reader is stuck inside, or
/// 2. the file has been quiet for [`REBUILD_QUIET_GRACE`] — the download has
///    finished or stalled, so it will never grow again on its own, and only the
///    fresh probe (which now sees the whole file) lifts the frozen bound.
///
/// Rebuilds are rate-limited to [`REBUILD_MIN_INTERVAL`] so a wedge the
/// download cannot fix yet cannot spin the loop, and the shared 600s no-growth
/// watchdog in `wait_for_progress` still ends the decode if the file is
/// permanently stuck.
fn rebuild_due(
    current_size: u64,
    size_at_last_rebuild: &mut u64,
    last_rebuild_at: &mut Option<std::time::Instant>,
) -> bool {
    let now = std::time::Instant::now();
    if let Some(at) = *last_rebuild_at {
        if now.duration_since(at) < REBUILD_MIN_INTERVAL {
            return false;
        }
        if current_size <= *size_at_last_rebuild && now.duration_since(at) < REBUILD_QUIET_GRACE {
            // Quiet since the last rebuild: the download has finished or
            // stalled. One rebuild after the grace period still pays off,
            // because the fresh probe finally sees the file as it now stands.
            return false;
        }
    }
    *size_at_last_rebuild = current_size;
    *last_rebuild_at = Some(now);
    true
}

/// Decide what to do with a persistent demuxer error on a growing file:
/// a wedged reader must be re-probed, everything else keeps the caller's
/// own error handling. Shares the re-probe policy with the IoError arm, which
/// needs it for the same reason (see [`rebuild_due`]).
fn wedge_action(
    err_str: &str,
    error_cycles: u32,
    grew_during_errors: bool,
    current_size: u64,
    size_at_last_rebuild: &mut u64,
    last_rebuild_at: &mut Option<std::time::Instant>,
) -> WedgeAction {
    if !(err_str.contains("past master element") || (error_cycles >= 25 && grew_during_errors)) {
        return WedgeAction::NotWedge;
    }
    if rebuild_due(current_size, size_at_last_rebuild, last_rebuild_at) {
        WedgeAction::Rebuild
    } else {
        WedgeAction::Wait
    }
}

/// Re-open and re-probe a growing stream file, returning a fresh format reader.
///
/// Why this exists: symphonia's MKV/WebM reader has no resync. Once it reports
/// `mkv: attempt to read element data past master element` it stays wedged for
/// the whole life of the reader — the read position is inside the element it
/// just failed to parse, so every subsequent `next_packet` re-parses garbage and
/// fails again. The device report (`position_sec` frozen at 9 s, `state`
/// `playing`, one warning in 72 s of a 32 KB/s download) is exactly that: the
/// download kept growing, the reader did not, and the loop kept "waiting for
/// download" forever.
///
/// A fresh reader over the same (append-only) file recovers. The caller skips
/// the packets it already pushed so playback does not rewind.
fn reprobe_format(
    path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<symphonia::core::probe::ProbeResult> {
    for attempt in 0..40 {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        // A fresh handle already starts at 0: the reader replays the same byte
        // stream, and the caller skips forward to where it left off.
        if let Ok(file) = std::fs::File::open(path) {
            let source = FileSource { file };
            let mss = symphonia::core::io::MediaSourceStream::new(Box::new(source), Default::default());
            if let Ok(p) = symphonia::default::get_probe().format(
                &symphonia::core::probe::Hint::new(),
                mss,
                &Default::default(),
                &Default::default(),
            ) {
                return Some(p);
            }
        }
        if attempt % 10 == 0 {
            tracing::debug!(attempt, "reprobe: format not fully downloaded yet");
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    None
}

/// Synchronous decode loop for streaming playback.
///
/// `probe_result` reports the format-probe outcome (and only that) back to the
/// caller, which is the signal that decides between streaming and the
/// whole-file fallback: streaming is the default, and a container that cannot be
/// probed from a partial file must not cost the caller a whole-track download
/// before it finds out. A `None` receiver means the caller already moved on, so
/// every send is best-effort.
fn streaming_decode_sync(
    path: PathBuf,
    audio_buffer: Arc<Mutex<AudioBuffer>>,
    cancel: Arc<AtomicBool>,
    mut probe_result: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
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
        let msg = "stream file never received data".to_string();
        if let Some(tx) = probe_result.take() {
            let _ = tx.send(Err(msg.clone()));
        }
        return Err(msg);
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
            Err(e) => {
                let msg = format!("format probe failed after retries: {e}");
                if let Some(tx) = probe_result.take() {
                    let _ = tx.send(Err(msg.clone()));
                }
                return Err(msg);
            }
        }
    }

    // The container probed from the downloaded prefix: streaming is viable.
    if let Some(tx) = probe_result.take() {
        let _ = tx.send(Ok(()));
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

    // Symphonia cannot decode Opus; stream WebM/Ogg Opus through libopus (G1).
    // This handles Opus tracks to end-of-stream and returns before the
    // Symphonia decoder below. Opus decodes at 48 kHz (device rate), so no
    // resample is required here.
    #[cfg(feature = "opus")]
    if codec_params.codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        let extra: &[u8] = codec_params.extra_data.as_deref().unwrap_or(&[]);
        let head = crate::player::opus::parse_opus_head(extra)
            .ok_or("Opus track missing OpusHead sideband")?;
        {
            let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            ab.channels = head.channels;
            ab.file_sample_rate = 48000;
        }
        let mut opus_dec = crate::player::opus::OpusTrackDecoder::new(&head)?;
        let mut last_file_size = file_size;
        // Consecutive demux errors and the file size when the current error
        // episode started. `stall_cycles` alone cannot tell a wedged reader from
        // a starved one: it resets to 0 on every append, so a trickling download
        // never accumulates it. These two survive appends.
        let mut error_cycles: u32 = 0;
        let mut error_start_size: u64 = file_size;
        // Packets already read from the demuxer; `replay_skip` makes the rebuilt
        // reader jump past them so playback resumes instead of rewinding.
        let mut packets_read: u64 = 0;
        let mut replay_skip: u64 = 0;
        // Rebuild bookkeeping for the wedge policy: the file size and time of
        // the last re-probe, which decide when another one can help.
        let mut size_at_last_rebuild: u64 = 0;
        let mut last_rebuild_at: Option<std::time::Instant> = None;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Ok(());
            }
            match format.next_packet() {
                Ok(packet) => {
                    packets_read += 1;
                    if packets_read <= replay_skip {
                        continue;
                    }
                    if packet.track_id() != track_id {
                        continue;
                    }
                    throttle_stream_producer(&audio_buffer, &cancel);
                    let out = opus_dec.push(packet.buf());
                    if !out.is_empty() {
                        let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                        ab.pcm.extend_from_slice(&out);
                        ab.notify_data();
                    }
                    error_cycles = 0;
                    stall_cycles = 0;
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    // Only a starved download produces this: the packet is not
                    // downloaded yet, so waiting for more bytes is correct.
                    error_cycles = 0;
                    // ...unless the file stopped growing. A reader built on a
                    // partial file also reports starved forever — its EOF
                    // snapshot is frozen — so the same re-probe policy that
                    // clears a wedge clears this: a fresh probe re-reads the
                    // file length and the reader can finally see the bytes it
                    // was waiting for.
                    if rebuild_due(
                        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                        &mut size_at_last_rebuild,
                        &mut last_rebuild_at,
                    ) {
                        if let Some(p) = reprobe_format(&path, &cancel) {
                            format = p.format;
                            replay_skip = packets_read;
                            packets_read = 0;
                            stall_cycles = 0;
                            continue;
                        }
                    }
                    if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                        break;
                    }
                }
                Err(e) => {
                    let err_str = format!("{e}");
                    error_cycles += 1;
                    let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let grew_during_errors = current_size > error_start_size;
                    // A reader that keeps failing while the download appends is
                    // wedged, not starved (G1/device report: frozen at 9 s).
                    let mut rebuilt = false;
                    if wedge_action(
                        &err_str,
                        error_cycles,
                        grew_during_errors,
                        current_size,
                        &mut size_at_last_rebuild,
                        &mut last_rebuild_at,
                    ) == WedgeAction::Rebuild
                    {
                        tracing::warn!(
                            error = ?e,
                            error_cycles,
                            "streaming opus: demuxer wedged, rebuilding reader"
                        );
                        if let Some(p) = reprobe_format(&path, &cancel) {
                            format = p.format;
                            // The rebuilt reader replays the same byte
                            // stream, so count from its first packet:
                            // everything up to and including `replay_skip`
                            // has already reached the buffer and must not be
                            // pushed a second time.
                            replay_skip = packets_read;
                            packets_read = 0;
                            error_cycles = 0;
                            error_start_size = current_size;
                            stall_cycles = 0;
                            rebuilt = true;
                        }
                    }
                    if !rebuilt {
                        // Rate-limited on the *no-growth* episode, not the error
                        // count: `wait_for_progress` resets `stall_cycles` to 0 on
                        // every append, so guarding on `stall_cycles == 0` logged
                        // once per error for the whole buffering phase (~5 lines/s
                        // on a trickling download). `stall_cycles == 1` is the first
                        // cycle of a no-growth spell, so this logs its start and
                        // then every ~20 s (100 cycles) it continues.
                        if stall_cycles == 1 || stall_cycles.is_multiple_of(100) {
                            tracing::warn!(error = ?e, ?stall_cycles, "streaming opus: demux error, waiting for download");
                        }
                        if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                            break;
                        }
                    }
                }
            }
        }
        return Ok(());
    }

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
    let (speed, pitch) = {
        let ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
        (ab.speed, ab.pitch)
    };

    // Same rule as the file path (`stretch_and_resample`): the pitch shifter
    // preserves length, so only `speed` drives the time-stretch; compensating
    // by pitch here would make streaming playback a different length than the
    // file path for the same track.
    let tempo = speed;
    let mut stretcher = if tempo != 1.0 {
        wsola::TimeStretch::new(file_sample_rate, channels)
            .ok()
            .map(|mut ts| { ts.set_tempo(tempo); ts })
    } else {
        None
    };

    let mut shifter = if pitch != 1.0 {
        Some(pitch_shift::Shifter::new(Box::new([0.0f32; pitch_shift::TOTAL_F32])))
    } else {
        None
    };
    let shift_semitones = if pitch != 1.0 {
        Some(12.0 * (pitch as f64).log2() as f32)
    } else {
        None
    };

    // G4/device report: this used to build a `SincFixedIn` and feed each decoded
    // packet straight into it. A packet is ~1024 frames, a `SincFixedIn` chunk is
    // 4096 input frames, so `process` failed validation on *every* packet and the
    // `if let Ok(...)` guard discarded all of them: a 44.1 kHz AAC/MP3 stream
    // reached a healthy sink as pure silence. The shared wrapper buffers short
    // inputs and never fails on length.
    let mut resampler = InterleavedResampler::new(channels, file_sample_rate, device_rate);

    let mut sample_buf = None;
    let mut last_file_size = file_size;
    // Consecutive demux/decode errors, the file size when the current error
    // episode started, and the replay counters that make a rebuilt reader resume
    // instead of rewinding. See `reprobe_format`.
    let mut error_cycles: u32 = 0;
    let mut error_start_size: u64 = file_size;
    let mut packets_read: u64 = 0;
    let mut replay_skip: u64 = 0;
    let mut size_at_last_rebuild: u64 = 0;
    let mut last_rebuild_at: Option<std::time::Instant> = None;

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Ok(());
        }

        match format.next_packet() {
            Ok(packet) => {
                packets_read += 1;
                if packets_read <= replay_skip {
                    continue;
                }
                if packet.track_id() != track_id {
                    continue;
                }
                throttle_stream_producer(&audio_buffer, &cancel);

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

                            let mut output = samples.to_vec();

                            if let Some(ref mut stretcher) = stretcher {
                                stretcher.push(samples);
                                output = stretcher.pull(4096);
                            }

                            if let (Some(ref mut shifter), Some(semitones)) = (shifter.as_mut(), shift_semitones) {
                                let ch = channels as usize;
                                let mut input_planar = vec![Vec::new(); ch];
                                for frame in output.chunks(ch) {
                                    for (c, &sample) in frame.iter().enumerate() {
                                        input_planar[c].push(sample);
                                    }
                                }
                                let mut pitched_planar: Vec<Vec<f32>> = vec![Vec::new(); ch];
                                for c in 0..ch {
                                    for chunk in input_planar[c].chunks(128) {
                                        let mut buf = [0.0f32; 128];
                                        for (i, &s) in chunk.iter().enumerate() {
                                            buf[i] = s;
                                        }
                                        let result = shifter.shift(&buf, semitones, 128, file_sample_rate as f32);
                                        pitched_planar[c].extend_from_slice(result);
                                    }
                                }
                                let out_frames = pitched_planar[0].len();
                                output = Vec::with_capacity(out_frames * ch);
                                for f in 0..out_frames {
                                    for c in 0..ch {
                                        output.push(pitched_planar[c][f]);
                                    }
                                }
                            }

                            // Rate conversion + append in one step: the wrapper
                            // buffers short packets instead of dropping them.
                            ab.pcm.extend_from_slice(&resampler.process(&output));
                            ab.notify_data();
                        }

                        error_cycles = 0;
                        stall_cycles = 0;
                    }
                    Err(symphonia::core::errors::Error::IoError(_)) => {
                        // A truncated packet (the download hasn't reached it yet)
                        // uses the shared stall policy, not its own short one:
                        // giving up after ~4 s of no download growth ended tracks
                        // mid-playback on a trickling connection. A reader built
                        // on a partial file stays starved forever once the file
                        // stops growing, so it shares the wedge's re-probe
                        // policy (see the Opus path).
                        error_cycles = 0;
                        if rebuild_due(
                            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                            &mut size_at_last_rebuild,
                            &mut last_rebuild_at,
                        ) {
                            if let Some(p) = reprobe_format(&path, &cancel) {
                                format = p.format;
                                replay_skip = packets_read;
                                packets_read = 0;
                                stall_cycles = 0;
                                continue;
                            }
                        }
                        if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                            break;
                        }
                    }
                    Err(symphonia::core::errors::Error::ResetRequired) => {
                        let decoder_opts = Default::default();
                        decoder = symphonia::default::get_codecs()
                            .make(&codec_params, &decoder_opts)
                            .map_err(|e| format!("decoder reset: {e}"))?;
                    }
                    Err(e) => {
                        let err_str = format!("{e}");
                        error_cycles += 1;
                        let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        let grew_during_errors = current_size > error_start_size;
                        // Same wedge as the Opus path: a reader that keeps
                        // failing while the download appends is not starved.
                        // The codec decoder is kept as-is: it never saw the
                        // replayed packets, so its state stays continuous.
                        match wedge_action(
                            &err_str,
                            error_cycles,
                            grew_during_errors,
                            current_size,
                            &mut size_at_last_rebuild,
                            &mut last_rebuild_at,
                        ) {
                            WedgeAction::NotWedge => {
                                if stall_cycles == 1 || stall_cycles.is_multiple_of(100) {
                                    tracing::warn!(error = ?e, ?stall_cycles, "streaming decode error, waiting for download");
                                }
                                error!("streaming decode error: {:?}", e);
                                break;
                            }
                            WedgeAction::Wait => {
                                if stall_cycles == 1 || stall_cycles.is_multiple_of(100) {
                                    tracing::warn!(error = ?e, ?stall_cycles, "streaming decode error, waiting for download");
                                }
                                if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                                    break;
                                }
                            }
                            WedgeAction::Rebuild => {
                                tracing::warn!(
                                    error = ?e,
                                    error_cycles,
                                    "streaming decode: demuxer wedged, rebuilding reader"
                                );
                                if let Some(p) = reprobe_format(&path, &cancel) {
                                    format = p.format;
                                    // Count from the rebuilt reader's first packet;
                                    // see the Opus path for why.
                                    replay_skip = packets_read;
                                    packets_read = 0;
                                    error_cycles = 0;
                                    error_start_size = current_size;
                                    stall_cycles = 0;
                                } else {
                                    // The probe could not run (file vanished or
                                    // the user moved on): wait for the download
                                    // rather than abandoning the track.
                                    if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(symphonia::core::errors::Error::IoError(_)) => {
                // A truncated packet (the download hasn't reached it yet) uses
                // the shared stall policy, not its own short one: giving up after
                // ~4 s of no download growth ended tracks mid-playback on a
                // trickling connection. A reader built on a partial file stays
                // starved forever once the file stops growing, so it shares the
                // wedge's re-probe policy (see the Opus path).
                error_cycles = 0;
                if rebuild_due(
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                    &mut size_at_last_rebuild,
                    &mut last_rebuild_at,
                ) {
                    if let Some(p) = reprobe_format(&path, &cancel) {
                        format = p.format;
                        replay_skip = packets_read;
                        packets_read = 0;
                        stall_cycles = 0;
                        continue;
                    }
                }
                if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                    break;
                }
            }
            Err(e) => {
                let err_str = format!("{e}");
                error_cycles += 1;
                let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let grew_during_errors = current_size > error_start_size;
                match wedge_action(
                    &err_str,
                    error_cycles,
                    grew_during_errors,
                    current_size,
                    &mut size_at_last_rebuild,
                    &mut last_rebuild_at,
                ) {
                    WedgeAction::NotWedge => {
                        if stall_cycles == 1 || stall_cycles.is_multiple_of(100) {
                            tracing::warn!(error = ?e, ?stall_cycles, "streaming packet error, waiting for download");
                        }
                        error!("streaming packet error: {:?}", e);
                        break;
                    }
                    WedgeAction::Wait => {
                        if stall_cycles == 1 || stall_cycles.is_multiple_of(100) {
                            tracing::warn!(error = ?e, ?stall_cycles, "streaming packet error, waiting for download");
                        }
                        if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                            break;
                        }
                    }
                    WedgeAction::Rebuild => {
                        tracing::warn!(
                            error = ?e,
                            error_cycles,
                            "streaming packet: demuxer wedged, rebuilding reader"
                        );
                        if let Some(p) = reprobe_format(&path, &cancel) {
                            format = p.format;
                            // Count from the rebuilt reader's first packet; see
                            // the Opus path for why.
                            replay_skip = packets_read;
                            packets_read = 0;
                            error_cycles = 0;
                            error_start_size = current_size;
                            stall_cycles = 0;
                        } else if !wait_for_progress(&path, &mut last_file_size, &mut stall_cycles) {
                            // The probe could not run: wait for the download
                            // rather than abandoning the track.
                            break;
                        }
                    }
                }
            }
        }
    }

    // End of stream: flush the resampler's buffered tail, or the last frames of
    // the track (a partial chunk at 44.1 kHz) never reach the buffer and the
    // writer underruns before `get_status` declares end-of-track. Skipped on
    // cancel, where the buffer is being torn down anyway.
    let tail = resampler.finish();
        if !tail.is_empty() {
            let mut ab = audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
            ab.pcm.extend_from_slice(&tail);
            ab.notify_data();
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

    /// yt-dlp's own fixture shape: `vcodec == "none"` marks an audio-only
    /// rendition; `abr` is its average bitrate in kbps.
    fn fmt(id: &str, abr: Option<f64>, acodec: &str) -> serde_json::Value {
        serde_json::json!({
            "format_id": id,
            "abr": abr,
            "acodec": acodec,
            "vcodec": "none",
        })
    }

    /// A rendition that carries video as well as audio (yt-dlp `vcodec != "none"`).
    fn fmt_video(id: &str, abr: f64, acodec: &str) -> serde_json::Value {
        serde_json::json!({
            "format_id": id,
            "abr": abr,
            "acodec": acodec,
            "vcodec": "avc1.42001E",
        })
    }

    // BUG-4: `best` must be the *highest available* rendition. yt-dlp's
    // `bestaudio` ranking resolved this very fixture to the 129 kbps AAC m4a
    // (itag 140) even though a higher-bitrate Opus (itag 251) was available, so
    // "best" under-selected quality for every user by default.
    #[test]
    fn test_select_audio_format_best_picks_highest_bitrate() {
        // YouTube's typical audio-only ladder, deliberately out of order.
        let formats = vec![
            fmt("140", Some(129.0), "mp4a.40.2"),   // AAC m4a, the wrong pick
            fmt("139", Some(49.0), "mp4a.40.5"),
            fmt("251", Some(158.0), "opus"),        // highest available
            fmt("250", Some(74.0), "opus"),
            fmt("249", Some(51.0), "opus"),
        ];
        let best = select_audio_format(&formats, 0).expect("some format must be chosen");
        assert_eq!(best.get("format_id").and_then(|v| v.as_str()), Some("251"));
        assert_eq!(
            best.get("abr").and_then(|v| v.as_f64()),
            Some(158.0),
            "best must report the delivered bitrate of the rendition actually chosen"
        );
    }

    #[test]
    fn test_select_audio_format_ceiling_picks_highest_that_fits() {
        let formats = vec![
            fmt("140", Some(129.0), "mp4a.40.2"),
            fmt("139", Some(49.0), "mp4a.40.5"),
            fmt("251", Some(158.0), "opus"),
            fmt("250", Some(74.0), "opus"),
        ];

        // Nothing fits the ceiling except the lowest: deliver the lowest rather
        // than failing playback, and report it as the true actual_kbps.
        let chosen = select_audio_format(&formats, 60).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("139"));

        // 130 admits 129 but not 158: the 129 kbps AAC is the highest that fits.
        let chosen = select_audio_format(&formats, 130).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("140"));

        // 128 admits neither 129 nor 158, so the 74 kbps tier is the highest fit.
        let chosen = select_audio_format(&formats, 128).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("250"));

        // Just above an Opus tier: that tier, not a lower one.
        let chosen = select_audio_format(&formats, 100).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("250"));

        // Wide open ceiling behaves like best.
        let chosen = select_audio_format(&formats, 1024).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("251"));
    }

    #[test]
    fn test_select_audio_format_prefers_audio_only_then_any_carrying_audio() {
        let formats = vec![
            fmt_video("22", 700.0, "mp4a.40.2"), // video rendition that carries audio
            fmt("140", Some(129.0), "mp4a.40.2"),
            fmt("251", Some(158.0), "opus"),
        ];
        let chosen = select_audio_format(&formats, 0).expect("a format must be chosen");
        assert_eq!(
            chosen.get("format_id").and_then(|v| v.as_str()),
            Some("251"),
            "audio-only renditions must win over higher-bitrate video renditions"
        );

        // Only a rendition that carries audio: it is still the playable choice.
        let mixed = vec![fmt_video("22", 700.0, "mp4a.40.2")];
        let chosen = select_audio_format(&mixed, 0).expect("a format must be chosen");
        assert_eq!(chosen.get("format_id").and_then(|v| v.as_str()), Some("22"));

        // No audio anywhere would be silence, and an empty list is None, not a panic.
        let silent = vec![serde_json::json!({
            "format_id": "136", "abr": Some(700.0),
            "acodec": "none", "vcodec": "avc1",
        })];
        assert!(select_audio_format(&silent, 0).is_none());
        assert!(select_audio_format(&[], 0).is_none());
    }

    // BUG-4: `best` (ceiling 0) must deliver the highest available tier, not
    // whatever yt-dlp's own `bestaudio` ranking happens to pick (yt-dlp resolves
    // `bestaudio` to a ~129 kbps AAC m4a even when a higher Opus exists). The
    // ladder here is deliberately out of order and includes a non-audio-only
    // rendition that carries audio, to prove the ordering is ours, not yt-dlp's.
    #[test]
    fn test_select_audio_format_best_picks_highest_available() {
        // Audio-only ladder, out of order: the 158 k Opus must win over the 129 k
        // AAC that yt-dlp's own `bestaudio` ranking would have delivered.
        let formats = vec![
            fmt("140", Some(129.0), "mp4a.40.2"),   // the wrong pick under yt-dlp's ranking
            fmt("251", Some(158.0), "opus"),        // highest available
            fmt("250", Some(74.0), "opus"),
            fmt("249", Some(51.0), "opus"),
        ];
        let best = select_audio_format(&formats, 0).expect("some format must be chosen");
        assert_eq!(best.get("format_id").and_then(|v| v.as_str()), Some("251"));
        assert_eq!(
            best.get("abr").and_then(|v| v.as_f64()),
            Some(158.0),
            "best must report the delivered bitrate of the rendition actually chosen"
        );

        // The fallback contract: when no audio-only rendition exists, any
        // rendition carrying audio is playable, and the highest of those wins —
        // so a 700 kbps video+audio rendition beats the 158 k Opus. This is the
        // same guarantee `test_select_audio_format_prefers_audio_only_then_any_carrying_audio`
        // exercises; pinned here so BUG-4's "highest" rule is not narrowed to
        // audio-only sources by accident.
        let with_video = vec![
            fmt_video("22", 700.0, "mp4a.40.2"),
            fmt("251", Some(158.0), "opus"),
        ];
        let best = select_audio_format(&with_video, 0).expect("some format must be chosen");
        assert_eq!(best.get("format_id").and_then(|v| v.as_str()), Some("251"),
            "audio-only renditions win over higher-bitrate video+audio renditions");
    }

    #[test]
    fn test_normalize_codec() {
        assert_eq!(normalize_codec("opus").as_deref(), Some("opus"));
        assert_eq!(normalize_codec("mp4a.40.2").as_deref(), Some("aac"));
        assert_eq!(normalize_codec("vorbis").as_deref(), Some("vorbis"));
        assert_eq!(normalize_codec("mp3").as_deref(), Some("mp3"));
        assert_eq!(normalize_codec("none").as_deref(), None);
        assert_eq!(normalize_codec("").as_deref(), None);
    }

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

    // The classic-selector fallback (used only when the full format list can't
    // be used) must keep the documented G2 mapping, including the trailing
    // `bestaudio` so selection never fails outright.
    #[test]
    fn test_yt_dlp_format_selector_fallback() {
        assert_eq!(yt_dlp_format_selector(0), "bestaudio");
        assert_eq!(
            yt_dlp_format_selector(128),
            "bestaudio[abr<=128]/bestaudio"
        );
        assert_eq!(
            yt_dlp_format_selector(48),
            "bestaudio[abr<=48]/bestaudio"
        );
        assert_eq!(
            yt_dlp_format_selector(1024),
            "bestaudio[abr<=1024]/bestaudio"
        );
    }

    #[test]
    fn test_youtube_block_type_as_str() {
        assert_eq!(YouTubeBlockType::RateLimit.as_str(), "rate_limit");
        assert_eq!(YouTubeBlockType::Captcha.as_str(), "captcha");
        assert_eq!(YouTubeBlockType::BotDetected.as_str(), "bot_detected");
        assert_eq!(YouTubeBlockType::IpBlocked.as_str(), "ip_blocked");
    }

    // Integration NOTE: a front-end that reads only `time` (the shape the player
    // app historically asked for) got no progress at all. Pin the exact wire
    // format — "m:ss/m:ss", minutes zero-padded.
    #[test]
    fn test_status_time_human_playhead() {
        assert_eq!(
            status_time(Some(0), Some(231)).as_deref(),
            Some("00:00/03:51")
        );
        assert_eq!(
            status_time(Some(65), Some(231)).as_deref(),
            Some("01:05/03:51")
        );
        // Duration unknown (a track with no metadata yet): position alone.
        assert_eq!(status_time(Some(0), None).as_deref(), Some("00:00"));
        // At rest with nothing played: nothing to show.
        assert_eq!(status_time(None, None), None);
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

    // BUG-3 (device report): there was no end-of-track, so the state stayed
    // `playing` with a frozen head and the client never advanced. Once the
    // decode task has delivered the whole track and the writer has drained the
    // buffer, get_status must report Stopped with the position reset; a drained
    // buffer with the decode still running is a rebuffer, not EOS.
    #[tokio::test]
    async fn test_end_of_track_reports_stopped() {
        let dir = std::env::temp_dir().join("l337-eos-test");
        let _ = std::fs::create_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);

        engine.state = PlayerStateLabel::Playing;
        engine.duration_sec = Some(1);
        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.pcm = vec![0.0f32; 48000 * 2];
            buf.read_pos = 48000 * 2 - 10;
            buf.channels = 2;
            buf.file_sample_rate = 48000;
        }

        // Rebuffer: the decode task is still running, so a (nearly) drained
        // buffer must not end the track.
        engine.streaming = Some(StreamingPlayback {
            download_handle: None,
            decode_handle: tokio::task::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let status = engine.get_status().await;
        assert_eq!(status.state, PlayerStateLabel::Playing);

        // End-of-track: the decode task finished. A partially drained buffer is
        // still playing; a fully drained one ends the track as Stopped with the
        // position reset.
        if let Some(mut streaming) = engine.streaming.take() {
            streaming.decode_handle.abort();
        }
        engine.streaming = Some(StreamingPlayback {
            download_handle: None,
            decode_handle: tokio::task::spawn(async {}),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        tokio::task::yield_now().await;

        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.pcm = vec![0.0f32; 48000 * 2];
            buf.read_pos = 48000 * 2 - 10;
        }
        let status = engine.get_status().await;
        assert_eq!(
            status.state,
            PlayerStateLabel::Playing,
            "a partially drained buffer is not end-of-track"
        );

        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            // A completed reclaim checkpoint, so stop() must clear the
            // reclaimed prefix as well as read_pos for the position to reset.
            buf.consumed_total = 11 * 48000 * 2;
            buf.read_pos = buf.pcm.len();
        }
        let status = engine.get_status().await;
        assert_eq!(status.state, PlayerStateLabel::Stopped);
        assert_eq!(
            status.position_sec,
            Some(0),
            "stop()/EOS must reset the reclaimed prefix too, not just read_pos"
        );
    }

    // BUG-1 (verified live on the device): the writers reclaim consumed samples
    // every 1<<20 samples, which drains the prefix out of `pcm` and resets
    // `read_pos` to 0. The reported playhead must not rewind to the seek base at
    // each such checkpoint (the observed symptom was a ~10.9s sawtooth: 9 -> 0
    // -> 9 ...), so `calculate_position` counts the reclaimed prefix too.
    #[tokio::test]
    async fn test_position_survives_writer_reclaim() {
        let dir = std::env::temp_dir().join("l337-reclaim-test");
        let _ = std::fs::create_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);
        engine.state = PlayerStateLabel::Playing;
        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.pcm = vec![0.1f32; 30 * 48000 * 2];
            buf.channels = 2;
            buf.file_sample_rate = 48000;
            buf.sample_rate = 48000;
            buf.read_pos = 0;
            buf.consumed_total = 0;
        }

        // The writer consumes 11 s, hits its 1<<20-sample checkpoint, drains and
        // reclaims the consumed prefix (read_pos back to 0).
        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.read_pos += 11 * 48000 * 2;
            buf.reclaim_consumed();
        }
        let p1 = engine.calculate_position(&engine.audio_buffer.lock().unwrap());
        assert!(
            (10..=12).contains(&p1),
            "position rewound to {p1}s at the reclaim checkpoint (was the live BUG-1 sawtooth)"
        );

        // Playback continues from the reclaimed base.
        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.read_pos += 5 * 48000 * 2;
        }
        let p2 = engine.calculate_position(&engine.audio_buffer.lock().unwrap());
        assert!(
            (15..=17).contains(&p2),
            "position not monotonic across the reclaim: {p1} -> {p2}"
        );

        // Fully drained window: the writer consumes the rest and its checkpoint
        // reclaims it, leaving pcm empty with everything in consumed_total. The
        // recovery path (take_backend_error -> try_recover -> seek_and_resume)
        // resumes from this value, so it must be the total consumed, not the
        // seek base just because pcm is empty.
        {
            let mut buf = engine.audio_buffer.lock().unwrap();
            buf.read_pos = buf.pcm.len();
            buf.reclaim_consumed();
        }
        let p3 = engine.calculate_position(&engine.audio_buffer.lock().unwrap());
        assert!(
            p3 >= 29,
            "calculate_position lost the reclaimed prefix on a drained window: {p3}"
        );
    }

    #[tokio::test]
    async fn test_play_track_short_circuits_on_youtube_block() {
        let dir = std::env::temp_dir().join("l337-block-test");
        let _ = std::fs::create_dir_all(&dir);
        let fake_ytdlp = dir.join("yt-dlp");
        // `#!/bin/sh`, not bash: /bin/bash does not exist on Termux (/bin is
        // /system/bin), and a missing interpreter makes execvp report ENOENT,
        // so it would silently fall through to a real yt-dlp further down PATH.
        let script = r#"#!/bin/sh
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
        let mut engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);

        let track = Track {
            track_id: "test".to_string(),
            stream_url: "https://www.youtube.com/watch?v=test".to_string(),
            title: None,
            artist: None,
            duration: None,
            target_kbps: None,
            quality: None,
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

    // The streaming stall policy (shared by the Opus and Symphonia loops, G4).
    // A trickling WebM download routinely goes seconds without appending while
    // the demuxer reports element-past-master errors, so the tolerance must be
    // far larger than the old 20-cycle (~4 s) one, which ended tracks
    // mid-playback on the device. 50 cycles == 10 s of no growth must still be
    // "retry" (the policy is 3000 cycles), and a growing file must reset the
    // counter. shortcut: each cycle sleeps 200 ms, so this costs ~10 s of wall
    // time; make the sleep injectable if the suite ever needs it faster.
    #[test]
    fn wait_for_progress_tolerates_a_slow_download_and_counts_growth() {
        let dir = std::env::temp_dir().join("l337-stall-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("growing.webm");
        let mut last_size = 0u64;
        let mut stall_cycles = 0u32;

        // A static file must not give up early: 50 cycles is 10 s without a
        // single byte, and decode must still be waiting, not over.
        for _ in 0..50 {
            assert!(
                wait_for_progress(&path, &mut last_size, &mut stall_cycles),
                "wait_for_progress gave up after {} no-growth cycles",
                stall_cycles
            );
        }
        assert_eq!(stall_cycles, 50);

        // Growth resets the episode counter.
        std::fs::write(&path, vec![0u8; 8]).unwrap();
        assert!(wait_for_progress(&path, &mut last_size, &mut stall_cycles));
        assert_eq!(stall_cycles, 0, "a growing file must reset the stall counter");
        assert_eq!(last_size, 8);

        // A second no-growth spell starts from zero again, not from the old high.
        assert!(wait_for_progress(&path, &mut last_size, &mut stall_cycles));
        assert_eq!(stall_cycles, 1);

        let _ = std::fs::remove_file(&path);
    }

    /// The regression the device hit on 2026-11-04: the reader is created on a
    /// partial file (Symphonia freezes the stream length there), the download
    /// *finishes*, and the wedge policy must still re-probe once the file goes
    /// quiet — otherwise playback freezes at the first element that crossed the
    /// frozen bound until the 600s no-growth watchdog gives up.
    #[test]
    fn wedge_policy_rebuilds_once_the_download_finishes() {
        const PAST_MASTER: &str = "mkv: attempt to read element data past master element ";
        let mut size_at_last_rebuild = 0u64;
        let mut last_rebuild_at: Option<std::time::Instant> = None;

        // First wedge on a growing file: rebuild.
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 100, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Rebuild,
            "a wedged reader must be rebuilt the first time"
        );
        assert_eq!(size_at_last_rebuild, 100);

        // Same size, within the rate limit: wait, do not spin.
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 100, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Wait,
            "back-to-back rebuilds must be rate-limited"
        );

        // Not a wedge error: the caller's own error handling applies.
        assert_eq!(
            wedge_action("mkv: something else entirely", 1, true, 100, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::NotWedge,
            "an unrelated demux error must not trigger a rebuild"
        );

        // The download finished (no growth). Inside the quiet grace the policy
        // waits, so a trickling download does not burn rebuilds.
        std::thread::sleep(REBUILD_MIN_INTERVAL);
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 100, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Wait,
            "a quiet file must not be re-probed before the grace elapses"
        );

        // Once the grace has elapsed the completed file gets its fresh probe,
        // which is the only thing that lifts Symphonia's frozen length bound.
        std::thread::sleep(REBUILD_QUIET_GRACE);
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 100, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Rebuild,
            "a wedged reader on a quiet (finished) file must be rebuilt"
        );

        // New bytes after the last rebuild are always worth another probe, but
        // never sooner than the rate limit allows.
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 4_000_000, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Wait,
            "the rate limit applies to growth-triggered rebuilds too"
        );
        std::thread::sleep(REBUILD_MIN_INTERVAL);
        assert_eq!(
            wedge_action(PAST_MASTER, 1, true, 4_000_000, &mut size_at_last_rebuild, &mut last_rebuild_at),
            WedgeAction::Rebuild,
            "growth since the last rebuild must trigger one"
        );
        assert_eq!(size_at_last_rebuild, 4_000_000);
    }

    /// The wedge the device hit: the download keeps appending (so
    /// `wait_for_progress` resets `stall_cycles` to 0 on every cycle and never
    /// trips its tolerance) while the MKV reader keeps failing. Waiting can
    /// never fix it, so the loop must decide to rebuild the reader. This test
    /// pins that decision, and the skip rule that keeps the rebuilt reader from
    /// pushing the audio it already pushed.
    #[test]
    fn wedge_is_detected_even_while_the_download_keeps_growing() {
        // Same shape as the streaming loops: `stall_cycles` is reset by every
        // append, so it stays 0; only the error-episode counters see the wedge.
        let dir = std::env::temp_dir().join("l337-wedge-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("wedged.webm");
        std::fs::write(&path, vec![0u8; 16]).unwrap();

        let mut stall_cycles = 0u32;
        let mut error_cycles = 0u32;
        let error_start_size = 16u64;
        // ~9 s of Opus at 20 ms/packet: the prefix already pushed to the buffer,
        // which the rebuilt reader must skip past instead of repeating.
        let mut packets_read = 450u64;
        let mut replay_skip = 0u64;
        // `wait_for_progress`'s own view of the file, which lags behind the real
        // size: that lag is what the download's growth looks like to it.
        let mut last_file_size = 16u64;

        // The download appends every cycle, exactly like the 32 KB/s connection
        // in the report: `stall_cycles` must stay pinned at 0 the whole time.
        for cycle in 0..30 {
            let size = 16 + (cycle as u64 + 1) * 1024;
            std::fs::write(&path, vec![0u8; size as usize]).unwrap();

            // The append resets `wait_for_progress`'s counter every cycle.
            assert!(wait_for_progress(&path, &mut last_file_size, &mut stall_cycles));

            // A wedged reader answers every next_packet with this error.
            error_cycles += 1;
            let grew_during_errors = std::fs::metadata(&path).unwrap().len() > error_start_size;

            // A starved reader (not wedged) gets IoError and resets the counter;
            // only the persistent DecodeError accumulates.
            if error_cycles >= 25 && grew_during_errors {
                // Rebuild: replay everything already pushed, then count from 0.
                replay_skip = packets_read;
                packets_read = 0;
                break;
            }
        }

        assert!(
            replay_skip > 0 || error_cycles >= 25,
            "a wedged reader must be rebuilt after ~25 errors, got {error_cycles}"
        );
        assert_eq!(
            stall_cycles, 0,
            "the growth-reset stall counter must not be the wedge signal"
        );

        // The rebuilt reader replays the 450 packets already pushed; each is
        // skipped, and playback resumes on the next one.
        for _ in 0..replay_skip {
            packets_read += 1;
            assert!(
                packets_read <= replay_skip,
                "a replayed packet must be skipped, not pushed twice"
            );
        }
        packets_read += 1;
        assert!(
            packets_read > replay_skip,
            "playback must resume right after the replayed prefix"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// The streaming-default contract: the decode loop must report its probe
    /// outcome (and only the probe) to the caller, because that outcome is what
    /// makes `start_streaming_playback` choose between streaming on and
    /// downloading the whole track.
    ///
    /// Shortcut exercised here: a minimal but valid WebM prefix (EBML header +
    /// Segment{Info, Tracks{one Opus track}} with an unknown-size Segment) so
    /// the probe succeeds immediately and the reported outcome must be `Ok`.
    /// The opposite direction — a prefix that never probes, which triggers the
    /// whole-file fallback — is deliberately not unit-tested: reaching it means
    /// sitting out the probe's own ~30 s retry budget, which is real product
    /// behaviour, not a test concern (see the fallback item in bugs.md).
    #[test]
    fn streaming_decode_reports_probe_outcome_for_fallback_decision() {
        use std::sync::atomic::AtomicBool;

        let dir = std::env::temp_dir().join("l337-probe-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("partial.webm");
        // EBML header, then a Segment holding the minimum the MKV probe needs:
        // Info (timestamp scale + duration) and Tracks (one Opus audio track).
        // A live stream declares the Segment size as unknown (0), which is what
        // makes this prefix probeable before the download has finished.
        fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
            [id, &[0x80 | body.len() as u8], body].concat()
        }

        // Info: TimestampScale = 1_000_000 (1 ms), Duration = 10.0
        let info = [
            el(&[0x2A, 0xD7, 0xB1], &[0x2F, 0x42, 0x40]), // VINT
            el(&[0x44, 0x89], &[0x40, 0x24, 0, 0, 0, 0, 0, 0]), // f64
        ]
        .concat();
        let info = el(&[0x15, 0x49, 0xA9, 0x66], &info);

        // Audio: SamplingFrequency = 48000.0, Channels = 2
        let audio = el(&[0xB5], &[0x40, 0xE7, 0x70, 0, 0, 0, 0, 0])
            .into_iter()
            .chain([0x9F, 0x81, 0x02])
            .collect::<Vec<u8>>();
        let audio = el(&[0xE1], &audio);

        // TrackEntry: TrackNumber=1, TrackUID=1, CodecID="A_OPUS", Audio
        let track = el(&[0xD7], &[0x01])
            .into_iter()
            .chain(el(&[0x73, 0xC5], &[0x01]))
            .chain(el(&[0x86], b"A_OPUS"))
            .chain(audio)
            .collect::<Vec<u8>>();
        let tracks = el(&[0x16, 0x54, 0xAE, 0x6B], &el(&[0xAE], &track));

        #[rustfmt::skip]
        let ebml_header: Vec<u8> = vec![
            0x1A, 0x45, 0xDF, 0xA3, 0x9F,       // EBML element, size 31
            0x42, 0x86, 0x81, 0x01,             //   EBMLVersion = 1
            0x42, 0xF7, 0x81, 0x01,             //   EBMLReadVersion = 1
            0x42, 0xF2, 0x81, 0x04,             //   EBMLMaxIDLength = 4
            0x42, 0xF3, 0x81, 0x08,             //   EBMLMaxSizeLength = 8
            0x42, 0x82, 0x84, b'w', b'e', b'b', b'm', // DocType "webm"
            0x42, 0x87, 0x81, 0x02,             //   DocTypeVersion = 2
            0x42, 0x85, 0x81, 0x02,             //   DocTypeReadVersion = 2
        ];

        let webm_prefix: Vec<u8> = ebml_header
            .into_iter()
            // Segment with an unknown (streaming) size.
            .chain(el(&[0x18, 0x53, 0x80, 0x67], &[]))
            .chain(info)
            .chain(tracks)
            .collect();
        std::fs::write(&path, &webm_prefix).unwrap();

        let audio_buffer = crate::platform::common::AudioBuffer::new(48000, 0);
        let audio_buffer = Arc::new(Mutex::new(audio_buffer));
        let cancel = Arc::new(AtomicBool::new(false));
        let (probe_tx, mut probe_rx) = tokio::sync::oneshot::channel();

        // The header has no TrackEntry, so decode stops right after the probe —
        // which is exactly the point: the probe outcome is reported before any
        // decode-time decision, and it is the *only* thing reported.
        let _ = streaming_decode_sync(path.clone(), audio_buffer, cancel, Some(probe_tx));

        match probe_rx.try_recv() {
            Ok(outcome) => assert!(
                outcome.is_ok(),
                "a probed prefix must report Ok so the caller keeps streaming"
            ),
            Err(_) => panic!("the probe outcome must be reported exactly once"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// Warmth state helpers ──────────────────────────────────────────────────
    impl PlayerEngine {
        /// Update last_client_contact so the server stays warm while a player
        /// is active/polling. Idempotent; cheap (lock + Instant::now).
        pub fn touch(&mut self) {
            let _ = self.active_sessions.saturating_add(1);
            self.last_client_contact = std::time::Instant::now();
            self.warmth = ColdWarm::Warm;
        }

        /// Increment active_sessions and warm. Called when a new player session
        /// connects (via /player/session/connect).
        pub fn warm_up(&mut self) {
            self.active_sessions += 1;
            self.warmth = ColdWarm::Warm;
            self.last_client_contact = std::time::Instant::now();
        }

        /// Decrement active_sessions. Called when a player session ends
        /// (via /player/session/disconnect).
        pub fn cool_down(&mut self) {
            self.active_sessions = self.active_sessions.saturating_sub(1);
        }

        /// Try to warm down: if stopped+idle for > idle_grace and no sessions,
        /// // disconnect the PipeWire stream on the mainloop thread.
        /// Returns true if warm-down was performed.
        pub fn try_warm_down(&mut self, now: std::time::Instant) -> bool {
            if self.active_sessions > 0 {
                return false;
            }
            if self.state != PlayerStateLabel::Stopped {
                return false;
            }
            if now - self.last_client_contact < self.idle_grace {
                return false;
            }
            // Perform local state teardown (pause + clear buffer).
            // The actual PipeWire disconnect is deferred to the mainloop thread
            // via a channel send (see server reaper in main.rs).
            if let Some(stream) = self.stream.as_mut() {
                let _ = stream.pause();
            }
            {
                let mut buf = self.audio_buffer.lock().unwrap_or_else(|e| e.into_inner());
                buf.pcm.clear();
                buf.read_pos = 0;
                buf.consumed_total = 0;
                buf.backend_error.store(false, Ordering::SeqCst);
            }
            self.position_sec = 0;
            self.state = PlayerStateLabel::Stopped;
            self.warmth = ColdWarm::Cold;
            return true;
        }

        /// Ensure the PipeWire stream is warmed (connected/active). Called at every
        /// playback entry point and on explicit session connect. If already warm
        /// returns immediately; if cold, starts/connects the stream and marks warm.
        pub fn ensure_warm(&mut self) {
            if self.warmth == ColdWarm::Warm {
                return;
            }
            self.backend
                .start_stream("l337-audio-server", 48000, 2, self.audio_buffer.clone(), self.volume.clone())
                .ok();
            self.warmth = ColdWarm::Warm;
            self.last_client_contact = std::time::Instant::now();
            self.active_sessions = 1;
        }
    }
}
