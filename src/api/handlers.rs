use crate::api::models::{PoolSettings, SpeedPayload, Track};
use crate::player::engine::{self, PlayerEngine};
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

pub type AppState = Arc<Mutex<PlayerEngine>>;

pub async fn play(State(state): State<AppState>, Json(track): Json<Track>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    engine.play_track(track).await;
    StatusCode::OK
}

pub async fn pause(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    match engine.state {
        crate::api::models::PlayerStateLabel::Playing => engine.pause(),
        crate::api::models::PlayerStateLabel::Paused => engine.resume(),
        _ => {}
    }
    StatusCode::OK
}

pub async fn next(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    engine.trigger_next().await;
    StatusCode::OK
}

pub async fn previous(State(state): State<AppState>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    engine.trigger_previous().await;
    StatusCode::OK
}

pub async fn cache_next(State(state): State<AppState>, Json(track): Json<Track>) -> impl IntoResponse {
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        let (dest, track_id) = {
            let engine = state_clone.lock().await;
            (engine.storage.get_active_slot_path("next"), track.track_id.clone())
        };

        info!("Starting background cache for track {} to next.stream", track_id);
        if let Err(e) = engine::download_stream(&track.stream_url, &dest).await {
            error!("Background cache failed for {}: {}", track_id, e);
        } else {
            let engine = state_clone.lock().await;
            let persistent_path = engine.storage.get_path_for_track(&track_id);
            let _ = tokio::fs::copy(&dest, &persistent_path).await;
            if let Ok(meta) = tokio::fs::metadata(&dest).await {
                engine.storage.update_access(&track_id, meta.len()).await;
                engine.storage.evict_if_needed(meta.len()).await;
            }
            info!("Background cache complete for {}", track_id);
        }
    });
    StatusCode::ACCEPTED
}

pub async fn cache_previous(State(state): State<AppState>, Json(track): Json<Track>) -> impl IntoResponse {
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        let (dest, track_id) = {
            let engine = state_clone.lock().await;
            (engine.storage.get_active_slot_path("prev"), track.track_id.clone())
        };

        info!("Starting background cache for track {} to prev.stream", track_id);
        if let Err(e) = engine::download_stream(&track.stream_url, &dest).await {
            error!("Background cache failed for {}: {}", track_id, e);
        } else {
            let engine = state_clone.lock().await;
            let persistent_path = engine.storage.get_path_for_track(&track_id);
            let _ = tokio::fs::copy(&dest, &persistent_path).await;
            if let Ok(meta) = tokio::fs::metadata(&dest).await {
                engine.storage.update_access(&track_id, meta.len()).await;
                engine.storage.evict_if_needed(meta.len()).await;
            }
            info!("Background cache complete for {}", track_id);
        }
    });
    StatusCode::ACCEPTED
}

pub async fn set_speed(State(state): State<AppState>, Json(payload): Json<SpeedPayload>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    engine.set_speed(payload.speed);
    StatusCode::OK
}

pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let engine = state.lock().await;
    let status = engine.get_status().await;
    Json(status)
}

pub async fn set_settings(State(state): State<AppState>, Json(settings): Json<PoolSettings>) -> impl IntoResponse {
    let mut engine = state.lock().await;
    engine.storage.max_pool_size = settings.max_disk_pool_bytes;
    StatusCode::OK
}
