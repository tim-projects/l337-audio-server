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

    fn write_minimal_wav(path: &PathBuf) {
        let sample_rate = 44100u32;
        let channels = 2u16;
        let duration_secs = 1u32;
        let num_samples = (sample_rate * duration_secs) as usize;
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;
        let data_bytes = (num_samples * channels as usize) as u32 * 2;
        let chunk_size = 36 + data_bytes;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&chunk_size.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for _ in 0..(num_samples * channels as usize) {
            bytes.extend_from_slice(&[0u8; 2]);
        }
        std::fs::write(path, bytes).unwrap();
    }

    async fn setup_state() -> AppState {
        let dir = std::env::temp_dir().join("l337-test-cache");
        let _ = std::fs::create_dir_all(&dir);
        let storage = StorageManager::new(500 * 1024 * 1024, Some(dir.clone())).await;
        let engine = PlayerEngine::new_dummy(storage);
        Arc::new(SendableEngine(Mutex::new(engine)))
    }

    #[tokio::test]
    async fn test_api_play() {
        let state = setup_state().await;
        let audio_path = std::env::temp_dir().join("l337-test-cache").join("test.wav");
        write_minimal_wav(&audio_path);
        let track = Track {
            track_id: "1".into(),
            stream_url: format!("file://{}", audio_path.display()),
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
