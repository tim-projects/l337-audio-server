mod api;
mod player;

use crate::api::handlers::{self, AppState};
use crate::player::engine::PlayerEngine;
use crate::player::storage::StorageManager;
use axum::{
    routing::{get, post, put},
    Router,
};
use rodio::OutputStream;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Deserialize)]
struct Settings {
    server: ServerSettings,
}

#[derive(Deserialize)]
struct ServerSettings {
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load config
    let settings = config::Config::builder()
        .add_source(config::File::with_name("config"))
        .build()
        .expect("Failed to load config.toml")
        .try_deserialize::<Settings>()
        .expect("Failed to parse config.toml");

    // Initialize storage and engine
    let storage = StorageManager::new(500 * 1024 * 1024).await; // 500MB
    let (stream, stream_handle) = match OutputStream::try_default() {
        Ok(res) => {
            (Some(res.0), Some(res.1))
        },
        Err(e) => {
            tracing::warn!("Audio output hardware failed ({}). Running in dummy output mode.", e);
            (None, None)
        }
    };
    
    let mut engine = PlayerEngine::new(storage, stream, stream_handle);
    if engine.sink.is_none() && engine.stream_handle.is_some() {
        tracing::warn!("Audio device found but could not initialize Sink. Running in dummy output mode.");
    } else if engine.sink.is_none() {
        tracing::info!("Running in dummy output mode.");
    } else {
        tracing::info!("Audio output initialized successfully.");
    }
    
    let shared_state: AppState = Arc::new(handlers::SendableEngine(Mutex::new(engine)));

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
    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .expect("Invalid address/port in config");
    tracing::info!("L337 Audio Server listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
