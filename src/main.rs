mod api;
mod platform;
mod player;
mod security;

use crate::api::handlers::{self, AppState};
use crate::player::engine::PlayerEngine;
use crate::player::storage::StorageManager;
use axum::{
    Router,
    routing::{get, post, put},
    Extension,
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    dummy: bool,
}

#[derive(Debug, Deserialize, Default)]
struct StorageSettings {
    #[serde(default)]
    max_cache_size_bytes: Option<u64>,
    #[serde(default)]
    cache_dir: Option<PathBuf>,
}

const DEFAULT_MAX_POOL: u64 = 256 * 1024 * 1024; // 256 MiB

/// Path to the Unix domain socket used for local IPC.
fn socket_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("l337")
        .join("l337-audio-server")
        .join("l337.sock")
}

/// Check whether a Unix socket at `path` is actively accepting connections.
#[cfg(unix)]
fn socket_is_active(path: &std::path::Path) -> bool {
    use std::os::unix::net::UnixStream;
    UnixStream::connect(path).is_ok()
}

/// Remove a stale Unix socket file if it exists and is not actively in use.
///
/// Returns `true` if the file was removed, `false` if it was actively serving.
#[cfg(unix)]
fn remove_stale_socket(path: &std::path::Path) -> bool {
    if path.exists() && socket_is_active(path) {
        return false;
    }
    let _ = std::fs::remove_file(path);
    true
}

#[cfg(not(unix))]
fn remove_stale_socket(_path: &std::path::Path) -> bool {
    true
}

fn parse_transport_cli() -> Option<String> {
    std::env::args().find_map(|a| a.strip_prefix("--transport=").map(|v| v.to_string()))
}

/// Default `server.ini` written when none exists, so the server always has a
/// usable configuration and never panics on a missing file.
const DEFAULT_CONFIG: &str = "[server]\nhost = \"127.0.0.1\"\nport = 1337\ndummy = false\ntransport = \"auto\"\n";

fn load_settings() -> Result<Settings, config::ConfigError> {
    let mut builder = config::Config::builder();
    builder = builder
        .add_source(config::File::with_name("/etc/l337-audio-server/config").required(false))
        .add_source(
            config::File::from(std::path::Path::new("/etc/l337-audio-server/server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        );

    if let Some(config_dir) = dirs::config_dir() {
        let xdg_path = config_dir.join("l337-audio-server").join("server.ini");
        builder = builder.add_source(
            config::File::from(xdg_path)
                .format(config::FileFormat::Ini)
                .required(false),
        );
    }

    builder = builder
        .add_source(
            config::File::from(std::path::Path::new("server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        )
        .add_source(config::Environment::with_prefix("L337").separator("__"));

    builder.build()?.try_deserialize::<Settings>()
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--version" || a == "-v") {
        println!("l337-audio-server {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Ensure a rustls crypto provider is selected (required before building TLS).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Initialize the platform-specific subsystem (runtime dirs, audio env, etc.).
    platform::init();

    // Acquire the single-instance lock before loading config or opening audio.
    // The guard is held for the lifetime of the process; dropping it on exit
    // removes the lock file.
    let _instance_guard = match platform::single_instance::InstanceLock::acquire() {
        Ok(lock) => lock,
        Err(e) => {
            tracing::error!("{}", e);
            std::process::exit(1);
        }
    };

    // Ensure a config file exists in the official config directory
    // (/etc/l337-audio-server) so a fresh install starts cleanly instead of
    // crashing on a missing [server] section.
    ensure_config_file();

    // Load configuration. The official location is /etc/l337-audio-server/
    // (systemd ConfigurationDirectory); fall back to XDG ~/.config/
    // then to a server.ini next to the binary (CWD) for local/dev runs.
    // Environment vars (L337__*) win last.
    let mut settings = load_settings().expect("Failed to load config");

    let max_pool = settings
        .storage
        .max_cache_size_bytes
        .unwrap_or(DEFAULT_MAX_POOL);

    let dummy_mode = settings.server.dummy || std::env::args().any(|a| a == "--dummy");

    // Determine transport: CLI flag > config file > default "auto"
    let cli_transport = parse_transport_cli();
    let effective_transport = cli_transport
        .or_else(|| settings.server.transport.clone())
        .unwrap_or_else(|| "auto".to_string());
    let use_socket = match effective_transport.as_str() {
        "socket" => true,
        "http" => false,
        "auto" => cfg!(unix),
        _ => cfg!(unix),
    };

    let storage = StorageManager::new(max_pool, settings.storage.cache_dir.clone()).await;
    let engine = if dummy_mode {
        tracing::warn!(
            "Running in DUMMY output mode. No audio will be produced. Testing only."
        );
        PlayerEngine::new_dummy(storage)
    } else {
        match PlayerEngine::new(storage) {
            Ok(engine) => engine,
            Err(e) => {
                tracing::error!("Failed to initialize audio device: {}", e);
                std::process::exit(1);
            }
        }
    };

    let shared_state: AppState = Arc::new(handlers::SendableEngine(Mutex::new(engine)));

    // Resolve the auth token: reuse configured value, else load/persist a stable
    // generated token so the client only needs to copy it once.
    // For Unix socket mode, auth is skipped (socket file permissions provide security).
    let token = match &settings.server.token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => load_or_create_token(),
    };
    let shared_token = Arc::new(Mutex::new(token.clone()));

    // Spawn SIGHUP config reload: re-read config + env, rotate token if changed.
    #[cfg(unix)]
    {
        let reload_token = shared_token.clone();
        let _reload_handle = tokio::spawn(async move {
            let mut signal = match tokio::signal::unix::signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("SIGHUP reload unavailable: {}", e);
                    return;
                }
            };
            while signal.recv().await.is_some() {
                tracing::warn!("SIGHUP received; reloading configuration");
                match load_settings() {
                    Ok(new_settings) => {
                        if let Some(new_token) = new_settings.server.token {
                            if !new_token.is_empty() {
                                let mut guard = reload_token.lock().await;
                                if *guard != new_token {
                                    tracing::warn!("Rotating auth token");
                                    *guard = new_token;
                                }
                            }
                        }
                    }
                    Err(e) => tracing::error!("Config reload failed: {}", e),
                }
            }
        });
    }

    // Build our application with routes
    let mut app = Router::new()
        .route("/health", get(handlers::health))
        .route("/setup", get(handlers::setup))
        .route("/version", get(handlers::version))
        .route("/", get(|| async { "L337 Audio Server" }))
        .route("/player/play", post(handlers::play))
        .route("/player/play/stream", post(handlers::upload_stream))
        .route("/player/pause", post(handlers::pause))
        .route("/player/next", post(handlers::next))
        .route("/player/previous", post(handlers::previous))
        .route("/player/cache/next", post(handlers::cache_next))
        .route("/player/cache/next/stream", post(handlers::upload_stream))
        .route("/player/cache/previous", post(handlers::cache_previous))
        .route(
            "/player/cache/previous/stream",
            post(handlers::upload_stream),
        )
        .route("/player/cache/lookup", post(handlers::cache_lookup))
        .route("/player/speed", post(handlers::set_speed))
        .route("/player/volume", post(handlers::set_volume))
        .route("/player/seek", post(handlers::seek))
        .route("/player/status", get(handlers::get_status))
        .route("/player/settings", put(handlers::set_settings))
        .with_state(shared_state)
        .layer(Extension(shared_token.clone()));

    // Skip auth layer for Unix socket mode (file permissions provide security).
    if !use_socket {
        app = app.layer(security::AuthLayer::new(token.clone()));
    }
    app = app.layer(tower_http::limit::RequestBodyLimitLayer::new(
        300 * 1024 * 1024,
    ));

    // Serve over Unix socket or TCP/HTTPS.
    #[cfg(unix)]
    if use_socket {
        let path = socket_path();
        if !remove_stale_socket(&path) {
            tracing::error!(
                "Unix socket {} is actively in use by another server. \
                 Refusing to remove it to prevent connection interception.",
                path.display()
            );
            std::process::exit(1);
        }
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let listener =
            std::os::unix::net::UnixListener::bind(&path).expect("Failed to bind Unix socket");
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
        tracing::info!("L337 Audio Server listening on Unix socket {}", path.display());
        axum_server::from_unix(listener)
            .expect("Failed to create axum server from Unix listener")
            .serve(app.into_make_service())
            .await
            .unwrap();
    } else {
        // TCP/HTTPS path (also used on non-Unix platforms)
        let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
            .parse()
            .expect("Invalid address/port in config");

        if !tcp_port_available(addr) {
            tracing::error!(
                "TCP port {} is already in use. Another instance may be running, \
                 or another service is bound to that port.",
                addr
            );
            std::process::exit(1);
        }

        match (
            settings.server.tls_cert.clone(),
            settings.server.tls_key.clone(),
        ) {
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

    #[cfg(not(unix))]
    {
        // Non-Unix platforms (Windows): always use TCP/HTTPS.
        let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
            .parse()
            .expect("Invalid address/port in config");

        if !tcp_port_available(addr) {
            tracing::error!(
                "TCP port {} is already in use. Another instance may be running, \
                 or another service is bound to that port.",
                addr
            );
            std::process::exit(1);
        }

        match (
            settings.server.tls_cert.clone(),
            settings.server.tls_key.clone(),
        ) {
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
}

fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

/// Create a default `server.ini` in the official config directory
/// (/etc/l337-audio-server) when none exists, so the server always has a valid
/// configuration on first run instead of panicking on a missing section. When
/// that directory is not present (local/dev runs) it falls back to the XDG
/// config directory, then to `server.ini` next to the binary (CWD).
fn ensure_config_file() {
    let etc_path = std::path::Path::new("/etc/l337-audio-server/server.ini");
    if etc_path.exists() {
        return;
    }
    if std::path::Path::new("/etc/l337-audio-server").is_dir() {
        match std::fs::write(etc_path, DEFAULT_CONFIG) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(etc_path, std::fs::Permissions::from_mode(0o600));
                }
                tracing::info!(
                    "No server.ini found; created a default at {}",
                    etc_path.display()
                )
            }
            Err(e) => tracing::warn!("Could not create default server.ini: {}", e),
        }
        return;
    }

    if let Some(config_dir) = dirs::config_dir() {
        let xdg_path = config_dir.join("l337-audio-server").join("server.ini");
        if !xdg_path.exists() {
            let _ = std::fs::create_dir_all(xdg_path.parent().unwrap());
            let _ = std::fs::write(&xdg_path, DEFAULT_CONFIG);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&xdg_path, std::fs::Permissions::from_mode(0o600));
            }
            tracing::info!(
                "No server.ini found; created a default at {}",
                xdg_path.display()
            );
            return;
        }
    }

    let cwd_path = std::path::Path::new("server.ini");
    if cwd_path.exists() {
        return;
    }
    match std::fs::write(cwd_path, DEFAULT_CONFIG) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(cwd_path, std::fs::Permissions::from_mode(0o600));
            }
            tracing::info!(
                "No server.ini found; created a default at {}",
                cwd_path.display()
            )
        }
        Err(e) => tracing::warn!("Could not create default server.ini: {}", e),
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
    } else if let Ok(meta) = std::fs::metadata(&path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() & 0o7777 | 0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
    tracing::warn!(
        "No [server] token configured. Generated + persisted a token at {}; add it to the \
         client's server_token setting: {}",
        path.display(),
        generated
    );
    generated
}

/// Probe whether `addr` can be bound. Returns `true` if the port is available.
fn tcp_port_available(addr: SocketAddr) -> bool {
    std::net::TcpListener::bind(addr).is_ok()
}
