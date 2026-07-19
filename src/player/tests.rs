#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::PlayerStateLabel;
    use crate::player::storage::StorageManager;
    use crate::player::engine::PlayerEngine;
    use rodio::DeviceSinkBuilder;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_storage_manager_eviction() {
        // Fresh, isolated cache dir so the test is not polluted by prior runs.
        let dir = std::env::temp_dir().join(format!("l337_test_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = StorageManager::new(1024, Some(dir.clone())).await; // 1KB limit
        storage.update_access("test1", 600).await;
        storage.update_access("test2", 600).await;
        // Eviction runs when a stream is committed (mirrors the real flow).
        storage.evict_if_needed(600).await;

        let size = storage.get_total_size().await;
        assert!(size <= 1024, "cache size {} exceeded 1024 cap", size);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_player_engine_state() {
        let dir = std::env::temp_dir().join(format!("l337_test_cache_state_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let (device_sink, mixer) = DeviceSinkBuilder::open_default_sink()
            .map(|sink| {
                let mixer = sink.mixer().clone();
                (Some(sink), Some(mixer))
            })
            .unwrap_or((None, None));
        let engine = PlayerEngine::new(storage, device_sink, mixer);
        assert_eq!(engine.state, PlayerStateLabel::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
