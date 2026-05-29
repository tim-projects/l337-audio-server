#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Track, PlayerStateLabel};
    use crate::player::storage::StorageManager;
    use crate::player::engine::PlayerEngine;

    #[tokio::test]
    async fn test_storage_manager_eviction() {
        let storage = StorageManager::new(1024).await; // 1KB limit for testing
        storage.update_access("test1", 600).await;
        storage.update_access("test2", 600).await;
        
        // Eviction should have happened
        let size = storage.get_total_size().await;
        assert!(size <= 1024);
    }

    #[tokio::test]
    async fn test_player_engine_state() {
        let storage = StorageManager::new(500 * 1024 * 1024).await;
        let (stream, stream_handle) = OutputStream::try_default().ok().map(|(s, h)| (Some(s), Some(h))).unwrap_or((None, None));
        let mut engine = PlayerEngine::new(storage, stream, stream_handle);
        
        assert_eq!(engine.state, PlayerStateLabel::Stopped);
    }
}
