use crate::api::models::CacheManifestEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use walkdir::WalkDir;

pub struct StorageManager {
    pub cache_dir: PathBuf,
    pub max_pool_size: u64,
    pub manifest: Arc<Mutex<HashMap<String, CacheManifestEntry>>>,
}

impl StorageManager {
    pub async fn new(max_pool_size: u64) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("./cache"))
            .join("l337player/cache");

        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir).await.expect("Failed to create cache directory");
        }

        let manifest_path = cache_dir.join("manifest.json");
        let manifest_data = if manifest_path.exists() {
            let content = fs::read_to_string(&manifest_path).await.unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Self {
            cache_dir,
            max_pool_size,
            manifest: Arc::new(Mutex::new(manifest_data)),
        }
    }

    pub async fn get_total_size(&self) -> u64 {
        WalkDir::new(&self.cache_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum()
    }

    pub async fn save_manifest(&self) {
        let manifest_path = self.cache_dir.join("manifest.json");
        let manifest = self.manifest.lock().await;
        if let Ok(content) = serde_json::to_string(&*manifest) {
            let _ = fs::write(manifest_path, content).await;
        }
    }

    pub async fn update_access(&self, track_id: &str, file_size: u64) {
        let mut manifest = self.manifest.lock().await;
        let entry = manifest.entry(track_id.to_string()).or_insert(CacheManifestEntry {
            track_id: track_id.to_string(),
            file_size,
            last_accessed: chrono::Utc::now().timestamp(),
            play_count: 0,
        });
        entry.last_accessed = chrono::Utc::now().timestamp();
        entry.play_count += 1;
        entry.file_size = file_size;
        drop(manifest);
        self.save_manifest().await;
    }

    pub async fn evict_if_needed(&self, incoming_size: u64) {
        let mut current_size = self.get_total_size().await;
        if current_size + incoming_size <= self.max_pool_size {
            return;
        }

        let mut manifest = self.manifest.lock().await;
        let mut entries: Vec<CacheManifestEntry> = manifest.values().cloned().collect();
        
        // Sort by last_accessed (oldest first) or play_count
        entries.sort_by(|a, b| a.last_accessed.cmp(&b.last_accessed));

        for entry in entries {
            if current_size + incoming_size <= self.max_pool_size {
                break;
            }

            // Protect active stream files
            if entry.track_id == "current" || entry.track_id == "next" || entry.track_id == "prev" {
                continue;
            }

            let file_path = self.cache_dir.join(&entry.track_id);
            if file_path.exists() {
                if let Ok(_) = fs::remove_file(&file_path).await {
                    current_size -= entry.file_size;
                    manifest.remove(&entry.track_id);
                }
            }
        }
        drop(manifest);
        self.save_manifest().await;
    }

    pub fn get_path_for_track(&self, track_id: &str) -> PathBuf {
        self.cache_dir.join(track_id)
    }

    pub fn get_active_slot_path(&self, slot: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.stream", slot))
    }
}
