use crate::api::models::{PlayerStateLabel, PlayerStatus, Track};
use crate::player::storage::StorageManager;
use futures_util::StreamExt;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

pub struct PlayerEngine {
    pub sink: Option<Sink>,
    pub stream_handle: Option<OutputStreamHandle>,
    pub storage: StorageManager,
    pub current_track: Option<Track>,
    pub state: PlayerStateLabel,
    pub speed: f32,
    pub volume: f32,
    pub duration_sec: Option<u64>,
}

impl PlayerEngine {
    pub fn new(storage: StorageManager, _stream: Option<OutputStream>, stream_handle: Option<OutputStreamHandle>) -> Self {
        let sink = if let Some(handle) = &stream_handle {
            Sink::try_new(handle).ok()
        } else {
            None
        };
        
        Self {
            sink,
            stream_handle,
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

        if let Err(e) = download_stream(&track.stream_url, &path).await {
            error!("Failed to download track: {}", e);
            return;
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
        if let Some(sink) = &self.sink {
            if let Ok(file) = File::open(&path) {
                let reader = BufReader::new(file);
                if let Ok(source) = Decoder::new(reader) {
                    self.duration_sec = source.total_duration().map(|d| d.as_secs());
                    sink.append(source);
                    sink.set_speed(self.speed);
                    sink.set_volume(self.volume);
                    sink.play();
                    self.state = PlayerStateLabel::Playing;
                }
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
            self.state = PlayerStateLabel::Paused;
        }
    }

    pub fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            sink.play();
            self.state = PlayerStateLabel::Playing;
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        if let Some(handle) = &self.stream_handle {
            self.sink = Sink::try_new(handle).ok();
        }
        self.state = PlayerStateLabel::Stopped;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        if let Some(sink) = &self.sink {
            sink.set_speed(speed);
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }

    /// Seek within the current track. rodio's `Sink` has no in-place seek, so we
    /// re-open the `current.stream` file, seek the decoder to `position`, stop the
    /// active sink, and append the seeked source. This restarts playback from the
    /// requested offset.
    pub fn seek(&mut self, position: u64) {
        let path = self.storage.get_active_slot_path("current");
        if let Some(sink) = &self.sink {
            if let Ok(file) = File::open(&path) {
                let reader = BufReader::new(file);
                if let Ok(mut source) = Decoder::new(reader) {
                    if source.try_seek(Duration::from_secs(position)).is_ok() {
                        sink.stop();
                        sink.append(source);
                        sink.set_speed(self.speed);
                        sink.set_volume(self.volume);
                        sink.play();
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

        // rodio's Sink does not expose the total duration of the active source,
        // so duration_sec is left to the client (which carries per-track metadata).
        let position_sec = if let Some(sink) = &self.sink {
            Some(sink.get_pos().as_secs())
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

    let response = reqwest::get(url).await?;
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
    Ok(())
}
