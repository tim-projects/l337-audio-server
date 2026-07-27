#[cfg(test)]
mod tests {
    use crate::api::models::PlayerStateLabel;
    use crate::player::engine::PlayerEngine;
    use crate::player::storage::StorageManager;

    #[tokio::test]
    async fn test_storage_manager_eviction() {
        let dir = std::env::temp_dir().join(format!("l337_test_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = StorageManager::new(1024, Some(dir.clone())).await;
        storage.update_access("test1", 600).await;
        storage.update_access("test2", 600).await;
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
        let engine = PlayerEngine::new_dummy(storage);
        assert_eq!(engine.state, PlayerStateLabel::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
