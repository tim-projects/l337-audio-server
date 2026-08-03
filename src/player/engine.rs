use crate::api::models::{PlayerStateLabel, PlayerStatus, Track};
use crate::player::storage::StorageManager;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::Resampler;
use rubato::SincFixedIn;
use rubato::SincInterpolationParameters;
use rubato::SincInterpolationType;
use rubato::WindowFunction;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

struct AudioBuffer {
    pcm: Vec<f32>,
    read_pos: usize,
    channels: u16,
    sample_rate: u32,
    file_sample_rate: u32,
    speed: f32,
}

impl AudioBuffer {
    fn new(sample_rate: u32, file_sample_rate: u32) -> Self {
        Self {
            pcm: Vec::new(),
            read_pos: 0,
            channels: 0,
            sample_rate,
            file_sample_rate,
            speed: 1.0,
        }
    }
}

pub struct PlayerEngine {
    stream: Option<cpal::Stream>,
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
}

// SAFETY: `cpal::Stream` is a handle to an OS audio stream. On all supported
// platforms it is safe to send it to another thread; cpal marks it `!Send` only
// because the underlying platform type is conservatively modeled. All other
// fields are `Send+Sync`, so `PlayerEngine` is `Send+Sync` in practice.
unsafe impl Send for PlayerEngine {}
unsafe impl Sync for PlayerEngine {}

impl PlayerEngine {
    pub fn new(storage: StorageManager) -> Self {
        let (stream, device_sample_rate) = Self::init_audio_device();

        let buffer = Arc::new(Mutex::new(AudioBuffer::new(device_sample_rate, 0)));
        let volume = Arc::new(Mutex::new(1.0));

        Self {
            stream,
            audio_buffer: buffer,
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
        }
    }

    pub fn new_dummy(storage: StorageManager) -> Self {
        Self {
            stream: None,
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
        }
    }

    fn init_audio_device() -> (Option<cpal::Stream>, u32) {
        let device = match cpal::default_host().default_output_device() {
            Some(d) => d,
            None => {
                eprintln!(
                    "FATAL: No audio output device available. A working audio output \
                     (e.g. PipeWire/ALSA) is required to serve audio. Re-run with --dummy \
                     to start without audio (testing only)."
                );
                std::process::exit(1);
            }
        };

        let mut supported = match device.supported_output_configs() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("FATAL: Failed to query audio output configs: {}", e);
                std::process::exit(1);
            }
        };

        let config = supported
            .find(|c| c.sample_format() == cpal::SampleFormat::F32)
            .or_else(|| supported.next())
            .expect("no supported audio output config");

        let sample_rate = config.max_sample_rate().0;
        let _sample_format = config.sample_format();
        let config: cpal::StreamConfig = config
            .with_sample_rate(cpal::SampleRate(sample_rate))
            .into();

        let audio_buffer = Arc::new(Mutex::new(AudioBuffer::new(sample_rate, 0)));
        let volume = Arc::new(Mutex::new(1.0));

        let stream = {
            let ab = audio_buffer.clone();
            let vol = volume.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if data.is_empty() {
                        return;
                    }
                    let mut buf = ab.lock().unwrap();
                    let available = buf.pcm.len().saturating_sub(buf.read_pos);
                    let to_copy = data.len().min(available);

                    let vol = *vol.lock().unwrap();
                    for (dst, src) in data.iter_mut().zip(buf.pcm[buf.read_pos..].iter()) {
                        *dst = src * vol;
                    }
                    buf.read_pos += to_copy;

                    for sample in &mut data[to_copy..] {
                        *sample = 0.0;
                    }
                },
                |err| error!("audio stream error: {}", err),
                None,
            )
        };

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() {
                    error!("Failed to start audio stream: {}", e);
                }
                (Some(s), sample_rate)
            }
            Err(e) => {
                eprintln!(
                    "FATAL: Audio device found but could not initialize a stream ({}). A working audio \
                     output is required. Re-run with --dummy to start without audio (testing only).",
                    e
                );
                std::process::exit(1);
            }
        }
    }

    pub async fn play_track(&mut self, track: Track) {
        self.stop();
        self.current_track = Some(track.clone());

        let path = self.storage.get_active_slot_path("current");
        info!(
            "play_track: downloading {} -> {}",
            track.stream_url,
            path.display()
        );

        if let Err(e) = download_stream(&track.stream_url, &path).await {
            error!("Failed to download track: {}", e);
            return;
        }

        match tokio::fs::metadata(&path).await {
            Ok(meta) => info!(
                "play_track: downloaded {} bytes to {}",
                meta.len(),
                path.display()
            ),
            Err(e) => error!("play_track: slot file missing after download: {}", e),
        }

        self.load_and_play("current").await;
        self.persist_slot(&track.track_id, &path).await;
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
        self.duration_sec = Some((pcm.len() / channels as usize) as u64);

        let device_rate = {
            let buf = self.audio_buffer.lock().unwrap();
            buf.sample_rate
        };

        let playback_pcm =
            resample_interleaved(&pcm, channels, file_sample_rate, device_rate, self.speed);

        let mut buf = self.audio_buffer.lock().unwrap();
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
        if let Some(ref stream) = self.stream {
            if let Err(e) = stream.pause() {
                error!("pause failed: {}", e);
            }
        }
        self.state = PlayerStateLabel::Paused;
    }

    pub fn resume(&mut self) {
        if let Some(ref stream) = self.stream {
            if let Err(e) = stream.play() {
                error!("resume failed: {}", e);
            }
        }
        self.state = PlayerStateLabel::Playing;
    }

    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
        let mut buf = self.audio_buffer.lock().unwrap();
        buf.pcm.clear();
        buf.read_pos = 0;
        drop(buf);
        self.position_sec = 0;
        self.state = PlayerStateLabel::Stopped;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
        let mut buf = self.audio_buffer.lock().unwrap();
        buf.speed = self.speed;
        drop(buf);
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume_val = volume.clamp(0.0, 1.0);
        *self.volume.lock().unwrap() = self.volume_val;
    }

    pub fn seek(&mut self, position: u64) {
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
            let buf = self.audio_buffer.lock().unwrap();
            buf.sample_rate
        };

        let playback_pcm =
            resample_interleaved(&pcm, channels, file_sample_rate, device_rate, self.speed);

        let mut buf = self.audio_buffer.lock().unwrap();
        buf.pcm = playback_pcm;
        buf.read_pos = 0;
        buf.channels = channels;
        buf.file_sample_rate = file_sample_rate;
        buf.speed = self.speed;
        drop(buf);

        self.position_sec = position;
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
            let buf = self.audio_buffer.lock().unwrap();
            if buf.pcm.is_empty() || buf.file_sample_rate == 0 {
                Some(self.position_sec)
            } else {
                let consumed = buf.read_pos as u64;
                let orig_frames = (consumed as f64 * buf.file_sample_rate as f64 * buf.speed as f64
                    / buf.sample_rate as f64) as u64;
                Some(orig_frames / buf.file_sample_rate as u64)
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
        }
    }
}

fn decode_to_pcm(
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
    let mut decoder = symphonia::default::get_codecs().make(&codec_params, &decoder_opts)?;

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

fn resample_interleaved(
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

    let output_planar = resampler.process(&input_planar, None).unwrap();

    let output_len = output_planar[0].len();
    let mut output = Vec::with_capacity(output_len * channels);
    for i in 0..output_len {
        for ch in 0..channels {
            output.push(output_planar[ch][i]);
        }
    }
    output
}

pub async fn download_stream(
    url: &str,
    dest: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

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

    if is_youtube_url(url) {
        return download_via_ytdlp(url, dest)
            .await
            .map_err(|e| format!("yt-dlp resolution failed for {url}: {e}").into());
    }

    let response = reqwest::get(url).await?;
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

async fn download_via_ytdlp(
    url: &str,
    dest: &PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::process::Command;

    let _ = tokio::fs::remove_file(dest).await;

    let status = Command::new("yt-dlp")
        .arg("--no-config")
        .arg("--no-warnings")
        .arg("-f")
        .arg("bestaudio[ext=m4a]/bestaudio[ext=mp3]/bestaudio")
        .arg("--no-playlist")
        .arg("-o")
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;

    if !status.success() {
        return Err(format!("yt-dlp exited with {status}").into());
    }
    let meta = tokio::fs::metadata(dest).await?;
    if meta.len() == 0 {
        return Err("yt-dlp produced an empty file".into());
    }
    Ok(())
}
