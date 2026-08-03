use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub track_id: String,
    pub stream_url: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpeedPayload {
    pub speed: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VolumePayload {
    pub volume: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SeekPayload {
    pub position: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PoolSettings {
    pub max_disk_pool_bytes: u64,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlayerStateLabel {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub state: PlayerStateLabel,
    pub volume: f32,
    pub speed: f32,
    pub current_track: Option<Track>,
    pub disk_pool_utilization_bytes: u64,
    pub next_cached: bool,
    pub prev_cached: bool,
    pub position_sec: Option<u64>,
    pub duration_sec: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheManifestEntry {
    pub track_id: String,
    pub file_size: u64,
    pub last_accessed: i64, // Unix timestamp
    pub play_count: u64,
}
