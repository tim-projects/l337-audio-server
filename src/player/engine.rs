use crate::api::models::{PlayerStateLabel, PlayerStatus, Track};
use crate::player::storage::StorageManager;
use futures_util::StreamExt;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

pub struct PlayerEngine {
    pub sink: Sink,
    _stream: OutputStream,
    pub stream_handle: OutputStreamHandle,
    pub storage: StorageManager,
    pub current_track: Option<Track>,
    pub state: PlayerStateLabel,
    pub speed: f32,
    pub volume: f32,
}

impl PlayerEngine {
    pub fn new(storage: StorageManager) -> Self {
        let (stream, stream_handle) = OutputStream::try_default().expect("Failed to open output stream");
        let sink = Sink::try_new(&stream_handle).expect("Failed to create sink");
        
        Self {
            sink,
            _stream: stream,
            stream_handle,
            storage,
            current_track: None,
            state: PlayerStateLabel::Stopped,
            speed: 1.0,
            volume: 1.0,
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
        
        // Save to persistent pool in background
        let track_id = track.track_id.clone();
        let source_path = path.clone();
        let dest_path = self.storage.get_path_for_track(&track_id);
        let _ = fs::copy(source_path, dest_path).await;
        
        if let Ok(meta) = fs::metadata(&path).await {
            self.storage.update_access(&track_id, meta.len()).await;
        }
    }

    pub async fn load_and_play(&mut self, slot: &str) {
        let path = self.storage.get_active_slot_path(slot);
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            if let Ok(source) = Decoder::new(reader) {
                self.sink.append(source);
                self.sink.set_speed(self.speed);
                self.sink.set_volume(self.volume);
                self.sink.play();
                self.state = PlayerStateLabel::Playing;
            }
        }
    }

    pub fn pause(&mut self) {
        self.sink.pause();
        self.state = PlayerStateLabel::Paused;
    }

    pub fn resume(&mut self) {
        self.sink.play();
        self.state = PlayerStateLabel::Playing;
    }

    pub fn stop(&mut self) {
        self.sink.stop();
        self.sink = Sink::try_new(&self.stream_handle).expect("Failed to recreate sink");
        self.state = PlayerStateLabel::Stopped;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.sink.set_speed(speed);
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

        PlayerStatus {
            state: self.state,
            volume: self.volume,
            speed: self.speed,
            current_track: self.current_track.clone(),
            disk_pool_utilization_bytes: utilization,
            next_cached,
            prev_cached,
        }
    }
}

pub async fn download_stream(url: &str, dest: &PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
