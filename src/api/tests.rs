#[cfg(test)]
mod tests {
    use crate::api::handlers::{AppState, SendableEngine, get_status, pause, play, seek, set_speed, set_volume, upload_stream, setup};
    use crate::api::models::{SeekPayload, SpeedPayload, Track, VolumePayload};
    use crate::player::engine::PlayerEngine;
    use crate::player::storage::StorageManager;
    use crate::security::AuthLayer;
    use axum::body::Body;
    use axum::extract::{Json, State};
    use axum::http::{HeaderMap, HeaderValue};
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
        let engine = PlayerEngine::new_dummy(storage, 24 * 1024 * 1024, 0);
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
            target_kbps: None,
            quality: None,
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

    #[tokio::test]
    async fn test_api_seek() {
        let state = setup_state().await;
        let audio_path = std::env::temp_dir().join("l337-test-cache").join("seek.wav");
        write_minimal_wav(&audio_path);
        let track = Track {
            track_id: "seek-track".into(),
            stream_url: format!("file://{}", audio_path.display()),
            title: Some("Seek".into()),
            artist: Some("Test".into()),
            duration: Some(1),
            target_kbps: None,
            quality: None,
        };
        let _ = play(State(state.clone()), Json(track)).await;

        let response = seek(State(state.clone()), Json(SeekPayload { position: 0 })).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
    }

    #[tokio::test]
    async fn test_api_set_speed() {
        let state = setup_state().await;
        let response = set_speed(State(state.clone()), Json(SpeedPayload { speed: 1.5, pitch: 1.0 })).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
        let status = get_status(State(state.clone())).await;
        let response = status.into_response();
        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["speed"], 1.5);
        assert_eq!(json["pitch"], 1.0);
    }

    #[tokio::test]
    async fn test_api_set_volume() {
        let state = setup_state().await;
        let response = set_volume(State(state.clone()), Json(VolumePayload { volume: 0.5 })).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );
        let status = get_status(State(state.clone())).await;
        let response = status.into_response();
        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["volume"], 0.5);
    }

    #[tokio::test]
    async fn test_api_upload_stream() {
        let state = setup_state().await;
        let audio_path = std::env::temp_dir().join("l337-test-cache").join("upload.wav");
        write_minimal_wav(&audio_path);
        let bytes = std::fs::read(&audio_path).expect("Failed to read WAV bytes");

        let mut headers = HeaderMap::new();
        headers.insert("X-Track-Id", HeaderValue::from_static("upload-track"));
        headers.insert("X-Title", HeaderValue::from_static("Upload Title"));
        headers.insert("X-Artist", HeaderValue::from_static("Upload Artist"));

        let response = upload_stream(
            State(state.clone()),
            axum::extract::OriginalUri(axum::http::Uri::from_static("/player/play/stream")),
            headers,
            Body::from(bytes),
        )
        .await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::OK
        );

        let status = get_status(State(state.clone())).await;
        let response = status.into_response();
        let body = response.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["state"], "playing");
        assert_eq!(json["current_track"]["track_id"], "upload-track");
    }

    #[tokio::test]
    async fn test_api_setup() {
        // When socket_mode is true (Unix socket), /setup returns the token.
        let auth_layer = AuthLayer::new("test-token-1234".to_string());
        let socket_mode = true;
        let response = setup(
            axum::Extension(auth_layer),
            axum::Extension(socket_mode),
        )
        .await;
        let resp = response.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = resp.into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["token"], "test-token-1234");
        assert_eq!(json["socket_mode"], true);

        // When socket_mode is false (TCP), /setup returns 404.
        let auth_layer = AuthLayer::new("test-token-1234".to_string());
        let socket_mode = false;
        let response = setup(
            axum::Extension(auth_layer),
            axum::Extension(socket_mode),
        )
        .await;
        let resp = response.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    /// A `track_id` must never escape the cache dir: it is used as a file name
    /// for both writing (cache/persist) and deleting (eviction).
    #[test]
    fn test_unsafe_track_ids_rejected() {
        for bad in [
            "",
            ".",
            "..",
            "../escape",
            "../../etc/passwd",
            "/tmp/absolute",
            "a/b",
            "a\\b",
            "trailing/",
            "nul\0id",
            &"x".repeat(129),
        ] {
            assert!(
                !crate::player::storage::is_safe_track_id(bad),
                "unsafe id must be rejected: {bad:?}"
            );
        }
        for good in ["abc123", "t-opus-ogg", "..hidden", "a.b-c_d", &"y".repeat(128)] {
            assert!(
                crate::player::storage::is_safe_track_id(good),
                "safe id must be accepted: {good:?}"
            );
        }
    }

    #[test]
    fn test_storage_rejects_paths_outside_cache_dir() {
        let cache = PathBuf::from("/nonexistent-cache-root");
        let storage = StorageManager {
            cache_dir: cache.clone(),
            max_pool_size: 1024,
            manifest: Arc::new(Mutex::new(std::collections::HashMap::new())),
        };
        assert_eq!(
            storage.get_path_for_track("../escape"),
            None,
            "relative traversal must not resolve to a cache path"
        );
        assert_eq!(
            storage.get_path_for_track("/etc/passwd"),
            None,
            "absolute path must not resolve to a cache path"
        );
        assert_eq!(
            storage.get_path_for_track("ok-id"),
            Some(cache.join("ok-id"))
        );
    }

    #[tokio::test]
    async fn test_play_rejects_traversal_track_id() {
        let state = setup_state().await;
        let track = Track {
            track_id: "../../escape".into(),
            stream_url: "file:///tmp/whatever".into(),
            title: None,
            artist: None,
            duration: None,
            target_kbps: None,
            quality: None,
        };
        let response = play(State(state), Json(track)).await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn test_upload_stream_rejects_traversal_track_id() {
        let state = setup_state().await;
        let mut headers = HeaderMap::new();
        headers.insert("X-Track-Id", HeaderValue::from_static("../escape"));
        let response = upload_stream(
            State(state),
            axum::extract::OriginalUri("/player/play/stream".parse().unwrap()),
            headers,
            Body::empty(),
        )
        .await;
        assert_eq!(
            response.into_response().status(),
            axum::http::StatusCode::BAD_REQUEST
        );
    }
}
