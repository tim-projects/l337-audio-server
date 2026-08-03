use crate::api::models::{PoolSettings, SeekPayload, SpeedPayload, Track, VolumePayload};
use crate::player::engine::{self, PlayerEngine};
use axum::{
    body::Body,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tracing::{error, info};

// Use a simple wrapper to make PlayerEngine safely shareable across tasks.
// tokio::sync::Mutex<PlayerEngine> is Send+Sync when PlayerEngine is Send+Sync,
// which it is naturally (cpal::Stream 0.15 is Send+Sync, all inner fields are
// primitives / Send+Sync types). No unsafe impl needed.
pub struct SendableEngine(pub Mutex<PlayerEngine>);

pub type AppState = Arc<SendableEngine>;

fn track_id_missing() -> impl IntoResponse {
    (
        StatusCode::BAD_REQUEST,
        "track_id and stream_url are required",
    )
        .into_response()
}

pub async fn play(State(state): State<AppState>, Json(track): Json<Track>) -> impl IntoResponse {
    if track.track_id.is_empty() || track.stream_url.is_empty() {
        return track_id_missing().into_response();
    }
    let mut engine = state.0.lock().await;
    engine.play_track(track).await;
    StatusCode::OK.into_response()
}

pub async fn pause(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    match engine.state {
        crate::api::models::PlayerStateLabel::Playing => engine.pause(),
        crate::api::models::PlayerStateLabel::Paused => engine.resume(),
        _ => {}
    }
    StatusCode::OK.into_response()
}

pub async fn next(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.trigger_next().await;
    StatusCode::OK.into_response()
}

pub async fn previous(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.trigger_previous().await;
    StatusCode::OK.into_response()
}

pub async fn cache_next(
    State(state): State<AppState>,
    Json(track): Json<Track>,
) -> impl IntoResponse {
    if track.track_id.is_empty() || track.stream_url.is_empty() {
        return track_id_missing().into_response();
    }
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        let (dest, track_id) = {
            let engine = state_clone.0.lock().await;
            (
                engine.storage.get_active_slot_path("next"),
                track.track_id.clone(),
            )
        };

        info!(
            "Starting background cache for track {} to next.stream",
            track_id
        );
        if let Err(e) = engine::download_stream(&track.stream_url, &dest).await {
            error!("Background cache failed for {}: {}", track_id, e);
        } else {
            let engine = state_clone.0.lock().await;
            let persistent_path = engine.storage.get_path_for_track(&track_id);
            let _ = tokio::fs::copy(&dest, &persistent_path).await;
            if let Ok(meta) = tokio::fs::metadata(&dest).await {
                engine.storage.update_access(&track_id, meta.len()).await;
                engine.storage.evict_if_needed(meta.len()).await;
            }
            info!("Background cache complete for {}", track_id);
        }
    });
    StatusCode::ACCEPTED.into_response()
}

pub async fn cache_previous(
    State(state): State<AppState>,
    Json(track): Json<Track>,
) -> impl IntoResponse {
    if track.track_id.is_empty() || track.stream_url.is_empty() {
        return track_id_missing().into_response();
    }
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        let (dest, track_id) = {
            let engine = state_clone.0.lock().await;
            (
                engine.storage.get_active_slot_path("prev"),
                track.track_id.clone(),
            )
        };

        info!(
            "Starting background cache for track {} to prev.stream",
            track_id
        );
        if let Err(e) = engine::download_stream(&track.stream_url, &dest).await {
            error!("Background cache failed for {}: {}", track_id, e);
        } else {
            let engine = state_clone.0.lock().await;
            let persistent_path = engine.storage.get_path_for_track(&track_id);
            let _ = tokio::fs::copy(&dest, &persistent_path).await;
            if let Ok(meta) = tokio::fs::metadata(&dest).await {
                engine.storage.update_access(&track_id, meta.len()).await;
                engine.storage.evict_if_needed(meta.len()).await;
            }
            info!("Background cache complete for {}", track_id);
        }
    });
    StatusCode::ACCEPTED.into_response()
}

pub async fn set_speed(
    State(state): State<AppState>,
    Json(payload): Json<SpeedPayload>,
) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.set_speed(payload.speed);
    StatusCode::OK.into_response()
}

pub async fn set_volume(
    State(state): State<AppState>,
    Json(payload): Json<VolumePayload>,
) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.set_volume(payload.volume);
    StatusCode::OK.into_response()
}

pub async fn seek(
    State(state): State<AppState>,
    Json(payload): Json<SeekPayload>,
) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.seek(payload.position);
    StatusCode::OK.into_response()
}

pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let engine = state.0.lock().await;
    let status = engine.get_status().await;
    Json(status).into_response()
}

pub async fn set_settings(
    State(state): State<AppState>,
    Json(settings): Json<PoolSettings>,
) -> impl IntoResponse {
    let mut engine = state.0.lock().await;
    engine.storage.max_pool_size = settings.max_disk_pool_bytes;
    StatusCode::OK.into_response()
}

/// Unauthenticated liveness probe used by clients to detect server reachability.
pub async fn health() -> impl IntoResponse {
    StatusCode::OK
}

/// Streaming upload of raw audio bytes into an active slot. The client pushes
/// audio the server cannot fetch itself (local file or transcoded stream).
/// Used by `/player/play/stream`, `/player/cache/next/stream`,
/// `/player/cache/previous/stream`.
pub async fn upload_stream(
    State(state): State<AppState>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> impl IntoResponse {
    // Derive the slot from the matched route path.
    let slot = match uri.path() {
        "/player/play/stream" => "current",
        "/player/cache/next/stream" => "next",
        "/player/cache/previous/stream" => "prev",
        _ => {
            return (StatusCode::BAD_REQUEST, "invalid upload endpoint").into_response();
        }
    };

    let track_id = match headers.get("X-Track-Id").and_then(|v| v.to_str().ok()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return (StatusCode::BAD_REQUEST, "X-Track-Id header required").into_response(),
    };

    let title = headers
        .get("X-Title")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let artist = headers
        .get("X-Artist")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let dest = {
        let engine = state.0.lock().await;
        engine.storage.get_active_slot_path(&slot)
    };

    // Stream the body to disk.
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&dest)
            .await?;
        let mut stream = body.into_data_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;

    if let Err(e) = result {
        error!("Stream upload to {}.stream failed: {}", slot, e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "upload failed").into_response();
    }

    let mut engine = state.0.lock().await;
    if slot == "current" {
        engine
            .play_pushed(&track_id, "current", title, artist)
            .await;
    } else {
        // next/prev are precached; record access so eviction accounts for them.
        if let Ok(meta) = tokio::fs::metadata(&dest).await {
            engine.storage.update_access(&track_id, meta.len()).await;
            engine.storage.evict_if_needed(meta.len()).await;
        }
    }
    StatusCode::OK.into_response()
}
