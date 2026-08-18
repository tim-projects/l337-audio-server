#[cfg(test)]
mod tests {
    use crate::player::engine::PlayerEngine;
    use crate::player::storage::StorageManager;
    use std::path::PathBuf;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[tokio::test]
    async fn test_player_engine_send_sync() {
        let storage = StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let engine = PlayerEngine::new_dummy(storage);
        assert_send::<PlayerEngine>();
        assert_sync::<PlayerEngine>();
    }
}
