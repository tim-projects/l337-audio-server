use crate::api::models::CacheManifestEntry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use walkdir::WalkDir;

pub struct StorageManager {
    pub cache_dir: PathBuf,
    pub max_pool_size: u64,
    pub manifest: Arc<Mutex<HashMap<String, CacheManifestEntry>>>,
    /// Running total of cached file bytes (OPT-15). Updated incrementally on
    /// insert and eviction instead of re-walking the directory on every call.
    pub total_size: u64,
    /// Debounce state for manifest writes (OPT-16). Only persist to disk when
    /// dirty and a minimum interval has elapsed, avoiding full JSON rewrites on
    /// every track access.
    pub(crate) manifest_dirty: bool,
    pub(crate) last_manifest_write: std::time::Instant,
}

/// Minimum interval between manifest disk writes (OPT-16).
const MANIFEST_WRITE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// A `track_id` becomes a file name inside the cache dir, so it must be one
/// plain path component. Absolute paths, `"/"`/`"\\\\"` separators, `.`/`..`
/// and NUL are rejected: a client-supplied id must never escape the cache dir.
pub fn is_safe_track_id(track_id: &str) -> bool {
    !track_id.is_empty()
        && track_id != "."
        && track_id != ".."
        && !track_id.contains('/')
        && !track_id.contains('\\')
        && !track_id.contains('\0')
        && track_id.len() <= 128
}

/// Default cache directory.
///
/// Honors the `CACHE_DIRECTORY` / `STATE_DIRECTORY` environment variables set
/// by systemd (so a hardened service writes to the provisioned, `l337`-owned
/// location). Falls back to `~/.cache/...`, then `./cache`.
fn default_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CACHE_DIRECTORY") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(dir) = std::env::var("STATE_DIRECTORY") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("cache");
        }
    }
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("./cache"))
        .join("l337")
        .join("l337-audio-server")
        .join("cache")
}

impl StorageManager {
    /// Create a `StorageManager`.
    ///
    /// `cache_dir` overrides the default `~/.cache/l337/l337-audio-server/cache/`.
    /// `max_pool_size` is the eviction cap in bytes (default 256 MiB on the caller side).
    pub async fn new(max_pool_size: u64, cache_dir: Option<PathBuf>) -> Self {
        let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);

        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir)
                .await
                .expect("Failed to create cache directory");
        }

        let manifest_path = cache_dir.join("manifest.json");
        let manifest_data = if manifest_path.exists() {
            let content = fs::read_to_string(&manifest_path).await.unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        // Compute initial total_size from manifest file sizes (single walk at
        // startup, not on every eviction).
        let total_size = manifest_data
            .values()
            .map(|e: &CacheManifestEntry| e.file_size)
            .sum();

        Self {
            cache_dir,
            max_pool_size,
            manifest: Arc::new(Mutex::new(manifest_data)),
            total_size,
            manifest_dirty: false,
            last_manifest_write: std::time::Instant::now(),
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

    pub async fn save_manifest(&mut self) {
        if !self.manifest_dirty {
            return;
        }
        if self.last_manifest_write.elapsed() < MANIFEST_WRITE_INTERVAL {
            return;
        }
        let manifest_path = self.cache_dir.join("manifest.json");
        let manifest = self.manifest.lock().await;
        if let Ok(content) = serde_json::to_string(&*manifest) {
            let _ = fs::write(manifest_path, content).await;
            self.manifest_dirty = false;
            self.last_manifest_write = std::time::Instant::now();
        }
    }

    pub async fn update_access(&mut self, track_id: &str, file_size: u64) {
        let mut manifest = self.manifest.lock().await;
        let is_new = manifest.get(track_id).is_none();
        let old_size = manifest.get(track_id).map(|e| e.file_size).unwrap_or(0);
        let entry = manifest
            .entry(track_id.to_string())
            .or_insert(CacheManifestEntry {
                track_id: track_id.to_string(),
                file_size,
                last_accessed: chrono::Utc::now().timestamp(),
                play_count: 0,
            });
        entry.last_accessed = chrono::Utc::now().timestamp();
        entry.play_count += 1;
        entry.file_size = file_size;
        drop(manifest);

        // Track total size incrementally: add new entries, adjust for size changes.
        if is_new {
            self.total_size = self.total_size.saturating_add(file_size);
        } else if file_size != old_size {
            self.total_size = self.total_size.saturating_add(file_size).saturating_sub(old_size);
        }

        // Mark manifest dirty for debounced disk write (OPT-16).
        self.manifest_dirty = true;
    }

    pub async fn evict_if_needed(&mut self, incoming_size: u64) {
        let mut current_size = self.total_size;
        if current_size + incoming_size <= self.max_pool_size {
            return;
        }

        let mut manifest = self.manifest.lock().await;
        let mut entries: Vec<CacheManifestEntry> = manifest.values().cloned().collect();

        // Evict least-played first, then oldest (least recently accessed).
        entries.sort_by(|a, b| {
            a.play_count
                .cmp(&b.play_count)
                .then(a.last_accessed.cmp(&b.last_accessed))
        });

        for entry in entries {
            if current_size + incoming_size <= self.max_pool_size {
                break;
            }

            // Protect active stream files
            if entry.track_id == "current" || entry.track_id == "next" || entry.track_id == "prev" {
                continue;
            }

            // The manifest is persisted data, so never trust its keys as paths:
            // only delete a file that resolves to a safe component inside our
            // own cache dir.
            let file_path = match self.get_path_for_track(&entry.track_id) {
                Some(p) => p,
                None => continue,
            };
            if file_path.exists() {
                if let Ok(_) = fs::remove_file(&file_path).await {
                    current_size = current_size.saturating_sub(entry.file_size);
                    self.total_size = self.total_size.saturating_sub(entry.file_size);
                    manifest.remove(&entry.track_id);
                }
            }
        }
        drop(manifest);
        self.manifest_dirty = true;
        self.save_manifest().await;
    }

    /// Cache file path for `track_id`, or `None` when the id is not a safe
    /// single path component (see [`is_safe_track_id`]).
    pub fn get_path_for_track(&self, track_id: &str) -> Option<PathBuf> {
        if !is_safe_track_id(track_id) {
            return None;
        }
        Some(self.cache_dir.join(track_id))
    }

    pub fn get_active_slot_path(&self, slot: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.stream", slot))
    }
}
