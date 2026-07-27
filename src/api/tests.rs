#[cfg(test)]
mod tests {
    use crate::api::handlers::{AppState, SendableEngine, get_status, pause, play};
    use crate::api::models::Track;
    use crate::player::engine::PlayerEngine;
    use crate::player::storage::StorageManager;
    use axum::extract::{Json, State};
    use axum::response::IntoResponse;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    async fn setup_state() -> AppState {
        let storage =
            StorageManager::new(500 * 1024 * 1024, Some(PathBuf::from("./test_cache"))).await;
        let engine = PlayerEngine::new_dummy(storage);
        Arc::new(SendableEngine(Mutex::new(engine)))
    }

    #[tokio::test]
    async fn test_api_play() {
        let state = setup_state().await;
        let track = Track {
            track_id: "1".into(),
            stream_url: "http://example.com/stream".into(),
            title: Some("Title".into()),
            artist: Some("Artist".into()),
            duration: Some(100),
        };
        let response = play(State(state.clone()), Json(track)).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_api_pause() {
        let state = setup_state().await;
        let response = pause(State(state.clone())).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_api_get_status() {
        let state = setup_state().await;
        let response = get_status(State(state.clone())).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
    }
}
