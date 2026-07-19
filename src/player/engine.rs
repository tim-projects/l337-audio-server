use crate::api::models::{PlayerStateLabel, PlayerStatus, Track};
use crate::player::storage::StorageManager;
use futures_util::StreamExt;
use rodio::{mixer::Mixer, Decoder, MixerDeviceSink, Player, Source};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

pub struct PlayerEngine {
    /// The active playback controller. `None` when no audio device is available
    /// (or in `--dummy` testing mode).
    pub player: Option<Player>,
    /// Handle to the mixer the `Player` is attached to. Recreating a `Player`
    /// after `stop()` requires this.
    pub mixer: Option<Mixer>,
    /// Keeps the OS audio stream alive; dropping it stops all sound.
    _device_sink: Option<MixerDeviceSink>,
    pub storage: StorageManager,
    pub current_track: Option<Track>,
    pub state: PlayerStateLabel,
    pub speed: f32,
    pub volume: f32,
    pub duration_sec: Option<u64>,
}

impl PlayerEngine {
    pub fn new(
        storage: StorageManager,
        device_sink: Option<MixerDeviceSink>,
        mixer: Option<Mixer>,
    ) -> Self {
        let player = mixer.as_ref().map(|m| Player::connect_new(m));

        Self {
            player,
            mixer,
            _device_sink: device_sink,
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            volume: 1.0,
            duration_sec: None,
        }
    }

    pub async fn play_track(&mut self, track: Track) {
        self.stop();
        self.current_track = Some(track.clone());

        let path = self.storage.get_active_slot_path("current");
        info!("play_track: downloading {} -> {}", track.stream_url, path.display());

        if let Err(e) = download_stream(&track.stream_url, &path).await {
            error!("Failed to download track: {}", e);
            return;
        }

        match tokio::fs::metadata(&path).await {
            Ok(meta) => info!("play_track: downloaded {} bytes to {}", meta.len(), path.display()),
            Err(e) => error!("play_track: slot file missing after download: {}", e),
        }

        self.load_and_play("current").await;
        self.persist_slot(&track.track_id, &path).await;
    }

    /// Play a slot (`current`/`next`/`prev`) that was filled by a client push
    /// upload. `track_id` is the client-supplied identity used for the persistent
    /// cache + eviction manifest.
    pub async fn play_pushed(&mut self, track_id: &str, slot: &str, title: Option<String>, artist: Option<String>) {
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

    /// Copy an active slot file into the persistent pool and record access.
    async fn persist_slot(&self, track_id: &str, slot_path: &PathBuf) {
        let dest_path = self.storage.get_path_for_track(track_id);
        let _ = fs::copy(slot_path, dest_path).await;
        if let Ok(meta) = fs::metadata(slot_path).await {
            self.storage.update_access(track_id, meta.len()).await;
        }
    }

    pub async fn load_and_play(&mut self, slot: &str) {
        let path = self.storage.get_active_slot_path(slot);
        if self.player.is_none() {
            error!("load_and_play({}): no Player available (audio device / sink init failed)", slot);
            return;
        }

        // Read the whole file into memory and decode from a `Cursor<Vec<u8>>`.
        // rodio 0.22's `Decoder` no longer panics on seek-during-init; the
        // in-memory cursor is still used as a defensive measure and to keep the
        // decode call cheap to run behind `catch_unwind`.
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                error!("load_and_play({}): cannot read {}: {}", slot, path.display(), e);
                return;
            }
        };

        // `Decoder::new` is a sync, panic-prone call (symphonia can hit an
        // `unreachable!()` on bad input). Run it on a blocking thread wrapped in
        // `catch_unwind` so a malformed file can never abort the server.
        let decode = tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Decoder::new(Cursor::new(bytes))
            }))
        })
        .await;

        match decode {
            Ok(Ok(Ok(source))) => {
                self.duration_sec = source.total_duration().map(|d| d.as_secs());
                if let Some(player) = &self.player {
                    player.append(source);
                    player.set_speed(self.speed);
                    player.set_volume(self.volume);
                    player.play();
                    self.state = PlayerStateLabel::Playing;
                    info!("load_and_play({}): Playing (duration {:?}s)", slot, self.duration_sec);
                }
            }
            Ok(Ok(Err(e))) => {
                error!("load_and_play({}): Decoder::new failed on {}: {:?}", slot, path.display(), e);
            }
            Ok(Err(_)) => {
                error!("load_and_play({}): decoder panicked on {} (unsupported/corrupt container)", slot, path.display());
            }
            Err(e) => {
                error!("load_and_play({}): decode task join error: {}", slot, e);
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(player) = &self.player {
            player.pause();
            self.state = PlayerStateLabel::Paused;
        }
    }

    pub fn resume(&mut self) {
        if let Some(player) = &self.player {
            player.play();
            self.state = PlayerStateLabel::Playing;
        }
    }

    pub fn stop(&mut self) {
        if let Some(player) = &self.player {
            player.stop();
        }
        // Recreate the Player so the mixer is ready for the next track. The
        // original Player is consumed by stop; `Player::connect_new` attaches a
        // fresh controller to the same mixer.
        self.player = self.mixer.as_ref().map(|m| Player::connect_new(m));
        self.state = PlayerStateLabel::Stopped;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        if let Some(player) = &self.player {
            player.set_speed(speed);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(player) = &self.player {
            player.set_volume(self.volume);
        }
    }

    /// Seek within the current track. rodio's `Player` has no in-place seek, so we
    /// re-open the `current.stream` file, seek the decoder to `position`, stop the
    /// active player, and append the seeked source. This restarts playback from
    /// the requested offset.
    pub fn seek(&mut self, position: u64) {
        let path = self.storage.get_active_slot_path("current");
        if let Some(player) = &self.player {
            if let Ok(bytes) = std::fs::read(&path) {
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Decoder::new(Cursor::new(bytes))
                }));
                if let Ok(Ok(mut source)) = decoded {
                    if source.try_seek(Duration::from_secs(position)).is_ok() {
                        player.stop();
                        player.append(source);
                        player.set_speed(self.speed);
                        player.set_volume(self.volume);
                        player.play();
                        self.state = PlayerStateLabel::Playing;
                    }
                }
            }
        }
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

        // rodio's Player does not expose the total duration of the active source,
        // so duration_sec is left to the client (which carries per-track metadata).
        let position_sec = if let Some(player) = &self.player {
            Some(player.get_pos().as_secs())
        } else {
            None
        };

        PlayerStatus {
            state: self.state,
            volume: self.volume,
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

pub async fn download_stream(url: &str, dest: &PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Same-device convenience: a local file or `file://` URL the server can read
    // is copied directly instead of fetched over the network.
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

    // YouTube / DASH sources: the raw URL often yields a container the audio
    // decoder cannot play (e.g. DASH/fMP4). When yt-dlp is available on the
    // server host, resolve it to a playable audio file directly. This keeps
    // the client stateless — it can just send the original YouTube URL.
    //
    // If yt-dlp is unavailable or fails, we MUST NOT fall back to fetching the
    // page URL directly — that returns HTML, which symphonia cannot decode and
    // which previously produced a silent "stopped". A YouTube resolution
    // failure is a hard error here.
    if is_youtube_url(url) {
        return download_via_ytdlp(url, dest)
            .await
            .map_err(|e| format!("yt-dlp resolution failed for {url}: {e}").into());
    }

    let response = reqwest::get(url).await?;
    if !response.status().is_success() {
        return Err(format!("fetch failed ({}): {}", response.status(), url).into());
    }

    // Reject non-media responses (e.g. an HTML error/login page) early so we
    // never write an undecodable file and silently end up "stopped".
    if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let ct = ct.to_str().unwrap_or("").to_lowercase();
        if ct.contains("text/html") {
            return Err(format!("refusing to download HTML response for {url} (content-type {ct})").into());
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

    // Final guard: if the file looks like HTML/text, reject it.
    if let Ok(bytes) = tokio::fs::read(dest).await {
        if bytes.len() >= 5 && (&bytes[0..5] == b"<!doc" || &bytes[0..5] == b"<html" || &bytes[0..5] == b"<?xml") {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(format!("downloaded content for {url} appears to be markup, not audio").into());
        }
    }
    Ok(())
}

/// True for YouTube watch/short/share URLs and raw googlevideo DASH URLs,
/// which the client may send and which require yt-dlp to turn into a
/// playable audio file.
fn is_youtube_url(url: &str) -> bool {
    url.contains("youtube.com/watch")
        || url.contains("youtu.be/")
        || url.contains("youtube.com/shorts/")
        || url.contains("googlevideo.com/videoplayback")
}

/// Resolve a YouTube/DASH URL to a playable audio file at `dest` using yt-dlp.
/// Returns Ok(()) only if yt-dlp ran successfully and produced a non-empty file.
async fn download_via_ytdlp(url: &str, dest: &PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::process::Command;

    // Remove any stale/partial file so a failed run can't leave a bad cache entry.
    let _ = tokio::fs::remove_file(dest).await;

    let status = Command::new("yt-dlp")
        .arg("--no-config") // ignore a possibly-broken user yt-dlp config
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
