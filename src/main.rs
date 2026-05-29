mod api;
mod player;

use crate::api::handlers::{self, AppState};
use crate::player::engine::PlayerEngine;
use crate::player::storage::StorageManager;
use axum::{
    routing::{get, post, put},
    Router,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Initialize storage and engine
    let storage = StorageManager::new(500 * 1024 * 1024).await; // 500MB
    let engine = PlayerEngine::new(storage);
    let shared_state: AppState = Arc::new(Mutex::new(engine));

    // Build our application with routes
    let app = Router::new()
        .route("/", get(|| async { "L337 Audio Server" }))
        .route("/player/play", post(handlers::play))
        .route("/player/pause", post(handlers::pause))
        .route("/player/next", post(handlers::next))
        .route("/player/previous", post(handlers::previous))
        .route("/player/cache/next", post(handlers::cache_next))
        .route("/player/cache/previous", post(handlers::cache_previous))
        .route("/player/speed", post(handlers::set_speed))
        .route("/player/status", get(handlers::get_status))
        .route("/player/settings", put(handlers::set_settings))
        .with_state(shared_state);

    // Run it
    let addr = SocketAddr::from(([127, 0, 0, 1], 1337));
    tracing::info!("L337 Audio Server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
