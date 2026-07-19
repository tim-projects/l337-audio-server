mod api;
mod player;
mod security;

use crate::api::handlers::{self, AppState};
use crate::player::engine::PlayerEngine;
use crate::player::storage::StorageManager;
use axum::{
    routing::{get, post, put},
    Router,
};
use rodio::DeviceSinkBuilder;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Deserialize)]
struct Settings {
    server: ServerSettings,
    #[serde(default)]
    storage: StorageSettings,
}

#[derive(Debug, Deserialize)]
struct ServerSettings {
    host: String,
    port: u16,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    tls_cert: Option<PathBuf>,
    #[serde(default)]
    tls_key: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Default)]
struct StorageSettings {
    #[serde(default)]
    max_cache_size_bytes: Option<u64>,
    #[serde(default)]
    cache_dir: Option<PathBuf>,
}

const DEFAULT_MAX_POOL: u64 = 256 * 1024 * 1024; // 256 MiB

/// Default `config.toml` written when none exists, so the server always has a
/// usable configuration and never panics on a missing file.
const DEFAULT_CONFIG: &str = "[server]\nhost = \"127.0.0.1\"\nport = 1337\n";

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Ensure a rustls crypto provider is selected (required before building TLS).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Ensure a config file exists in the official config directory
    // (/etc/l337-audio-server) so a fresh install starts cleanly instead of
    // crashing on a missing [server] section.
    ensure_config_file();

    // Load configuration. The official location is /etc/l337-audio-server/
    // (systemd ConfigurationDirectory); fall back to a config.toml next to the
    // binary (CWD) for local/dev runs. Environment vars (L337__*) win last.
    let settings = config::Config::builder()
        .add_source(config::File::with_name("/etc/l337-audio-server/config").required(false))
        .add_source(config::File::with_name("config").required(false))
        .add_source(config::Environment::with_prefix("L337").separator("__"))
        .build()
        .expect("Failed to build config")
        .try_deserialize::<Settings>()
        .expect("Failed to parse config");

    let max_pool = settings
        .storage
        .max_cache_size_bytes
        .unwrap_or(DEFAULT_MAX_POOL);

    // Initialize storage and engine
    // `--dummy` (testing only): skip real audio hardware entirely and run
    // without a Sink. Never enabled by config — must be an explicit flag.
    let dummy_mode = std::env::args().any(|a| a == "--dummy");

    // Initialize storage and engine. A real audio server always uses the host
    // audio hardware (e.g. PipeWire/ALSA). Dummy mode skips it on purpose.
    let storage = StorageManager::new(max_pool, settings.storage.cache_dir.clone()).await;
    let (device_sink, mixer) = if dummy_mode {
        tracing::warn!("Running in DUMMY output mode (--dummy). No audio will be produced. Testing only.");
        (None, None)
    } else {
        match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let mixer = sink.mixer().clone();
                (Some(sink), Some(mixer))
            }
            Err(e) => {
                eprintln!(
                    "FATAL: No audio output device available ({}). A working audio output \
                     (e.g. PipeWire/ALSA) is required to serve audio. Re-run with --dummy \
                     to start without audio (testing only).",
                    e
                );
                std::process::exit(1);
            }
        }
    };

    let engine = PlayerEngine::new(storage, device_sink, mixer);
    if !dummy_mode {
        if engine.player.is_none() {
            eprintln!(
                "FATAL: Audio device found but could not initialize a Player. A working audio \
                 output is required. Re-run with --dummy to start without audio (testing only)."
            );
            std::process::exit(1);
        }
        tracing::info!("Audio output initialized successfully.");
    }

    let shared_state: AppState = Arc::new(handlers::SendableEngine(Mutex::new(engine)));

    // Resolve the auth token: reuse configured value, else load/persist a stable
    // generated token so the client only needs to copy it once.
    let token = match &settings.server.token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => load_or_create_token(),
    };

    // Build our application with routes
    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/", get(|| async { "L337 Audio Server" }))
        .route("/player/play", post(handlers::play))
        .route("/player/play/stream", post(handlers::upload_stream))
        .route("/player/pause", post(handlers::pause))
        .route("/player/next", post(handlers::next))
        .route("/player/previous", post(handlers::previous))
        .route("/player/cache/next", post(handlers::cache_next))
        .route("/player/cache/next/stream", post(handlers::upload_stream))
        .route("/player/cache/previous", post(handlers::cache_previous))
        .route("/player/cache/previous/stream", post(handlers::upload_stream))
        .route("/player/speed", post(handlers::set_speed))
        .route("/player/volume", post(handlers::set_volume))
        .route("/player/seek", post(handlers::seek))
        .route("/player/status", get(handlers::get_status))
        .route("/player/settings", put(handlers::set_settings))
        .with_state(shared_state)
        .layer(security::AuthLayer::new(token));

    // Run it (optionally over TLS)
    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .expect("Invalid address/port in config");

    match (settings.server.tls_cert.clone(), settings.server.tls_key.clone()) {
        (Some(cert), Some(key)) => {
            tracing::info!("L337 Audio Server listening with TLS on https://{}", addr);
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .expect("invalid TLS cert/key");
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await
                .unwrap();
        }
        _ => {
            // No TLS configured: auto-generate a self-signed cert so the server is
            // encrypted by default (works for LAN + WWAN). The client must trust
            // the fingerprint / disable verification for self-signed.
            let host = settings.server.host.clone();
            let certified = security::generate_self_signed(&host);
            let tls = security::rustls_config(certified);
            tracing::warn!(
                "No TLS cert configured. Auto-generated a self-signed certificate; \
                 server is available at https://{}. Configure a trusted cert in \
                 [server] tls_cert/tls_key for production.",
                addr
            );
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await
                .unwrap();
        }
    }
}

fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Create a default `config.toml` in the official config directory
/// (/etc/l337-audio-server) when none exists, so the server always has a valid
/// configuration on first run instead of panicking on a missing section. When
/// that directory is not present (local/dev runs) it falls back to a
/// config.toml next to the binary (CWD).
fn ensure_config_file() {
    let etc_path = std::path::Path::new("/etc/l337-audio-server/config.toml");
    if etc_path.exists() {
        return;
    }
    if std::path::Path::new("/etc/l337-audio-server").is_dir() {
        match std::fs::write(etc_path, DEFAULT_CONFIG) {
            Ok(()) => {
                tracing::info!("No config.toml found; created a default at {}", etc_path.display())
            }
            Err(e) => tracing::warn!("Could not create default config.toml: {}", e),
        }
        return;
    }

    let cwd_path = std::path::Path::new("config.toml");
    if cwd_path.exists() {
        return;
    }
    match std::fs::write(cwd_path, DEFAULT_CONFIG) {
        Ok(()) => tracing::info!("No config.toml found; created a default at {}", cwd_path.display()),
        Err(e) => tracing::warn!("Could not create default config.toml: {}", e),
    }
}

/// Load a previously generated token from the cache dir, or create + persist one.
/// Keeps the token stable across restarts when none is configured explicitly.
fn load_or_create_token() -> String {
    // Prefer the systemd-provided STATE_DIRECTORY (persistent, l337-owned),
    // falling back to ~/.cache/... for dev/desktop runs.
    let dir = if let Ok(dir) = std::env::var("STATE_DIRECTORY") {
        if !dir.is_empty() {
            PathBuf::from(dir)
        } else {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("l337")
                .join("l337-audio-server")
        }
    } else {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("l337")
            .join("l337-audio-server")
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("server_token.txt");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim().to_string();
        if !t.is_empty() {
            tracing::warn!(
                "Using persisted auth token from {}. Add it to the client's server_token setting.",
                path.display()
            );
            return t;
        }
    }
    let generated = generate_token();
    if let Err(e) = std::fs::write(&path, &generated) {
        tracing::warn!("Could not persist token to {}: {}", path.display(), e);
    }
    tracing::warn!(
        "No [server] token configured. Generated + persisted a token at {}; add it to the \
         client's server_token setting: {}",
        path.display(),
        generated
    );
    generated
}
