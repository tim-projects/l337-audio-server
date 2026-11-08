mod api;
mod auth_challenge;
mod platform;
mod player;
mod rate_limit;
mod secrets_fs;
mod security;

use crate::api::handlers::{self, AppState};
use crate::auth_challenge::ChallengeState;
use crate::player::engine::PlayerEngine;
use crate::player::storage::StorageManager;
use crate::rate_limit::RateLimiter;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
use tokio::sync::Mutex;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, Deserialize, Default)]
struct Settings {
    #[serde(default)]
    server: ServerSettings,
    #[serde(default)]
    storage: StorageSettings,
}

#[derive(Debug, Deserialize, Default)]
struct ServerSettings {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
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
    socket_mode: Option<String>,
    #[serde(default)]
    dummy: bool,
    #[serde(default)]
    buffer_max_bytes: Option<u64>,
    #[serde(default)]
    default_target_kbps: Option<u32>,
    /// Rust log level for `tracing` (`error`/`warn`/`info`/`debug`/`trace`).
    /// An explicit `RUST_LOG` env var wins over this; an unset/invalid value
    /// keeps the crate default (`warn`). `off` is deliberately NOT accepted
    /// here: only the unit file owner can silence the server entirely, via
    /// `RUST_LOG=off`, so a config edit cannot leave a running install with no
    /// diagnostics and no record that logging was turned off.
    #[serde(default)]
    loglevel: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct StorageSettings {
    #[serde(default)]
    max_cache_size_bytes: Option<u64>,
    #[serde(default)]
    cache_dir: Option<PathBuf>,
}

const DEFAULT_MAX_POOL: u64 = 256 * 1024 * 1024; // 256 MiB

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    1337
}

fn socket_mode_from_settings(settings: &ServerSettings) -> u32 {
    settings
        .socket_mode
        .as_deref()
        .and_then(|s| u32::from_str_radix(s.trim(), 8).ok())
        .map(|m| m & 0o777)
        .unwrap_or(0o600)
}

/// Path to the Unix domain socket used for local IPC.
fn socket_path() -> PathBuf {
    platform::common::runtime_dir().join("l337.sock")
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
///
/// `buffer_max_bytes` is the *rebuffer window* (a few seconds of decoded PCM),
/// not a whole track: consumed samples are reclaimed as playback advances (G4),
/// so a generous window never OOMs a long track. `default_target_kbps = 0` means
/// `best` (no ceiling) — see G2.
const DEFAULT_CONFIG: &str = "[server]\nhost = \"127.0.0.1\"\nport = 1337\ndummy = false\ntransport = \"auto\"\n# Rebuffer window (~24 MiB), not a whole-track buffer (see G4).\nbuffer_max_bytes = 25165824\n# Bandwidth ceiling in kbps; 0 = best / no ceiling (see G2).\ndefault_target_kbps = 0\n# Rust log level: error | warn | info | debug | trace. Default verbosity is\n# warn (errors and warnings only); set debug while hunting bugs. An explicit\n# RUST_LOG environment variable overrides this (RUST_LOG wins, see init_tracing).\nloglevel = \"warn\"\n";

fn load_settings() -> Result<Settings, config::ConfigError> {
    let mut builder = config::Config::builder();
    builder = builder
        .add_source(config::File::with_name("/etc/l337-audio-server/config").required(false))
        .add_source(
            config::File::from(std::path::Path::new("/etc/l337-audio-server/server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        );

    builder = builder
        .add_source(
            config::File::from(std::path::Path::new("server.ini"))
                .format(config::FileFormat::Ini)
                .required(false),
        )
        .add_source(config::Environment::with_prefix("L337").separator("__"));

    builder.build()?.try_deserialize::<Settings>()
}

/// The tracing levels accepted for the `loglevel` config key (case-insensitive).
/// `off` is intentionally absent: silencing the server from a config file the
/// service principal can write would leave a running install with no
/// diagnostics and no record of it. `RUST_LOG=off` (owned by whoever controls
/// the environment) remains the way to silence it.
fn is_tracing_level(level: &str) -> bool {
    matches!(
        level.to_ascii_lowercase().as_str(),
        "error" | "warn" | "info" | "debug" | "trace"
    )
}

/// The `[server] loglevel` key resolved: the level to use, whether the value
/// was unrecognized (warn after the subscriber exists), and whether the key was
/// meaningfully set (drives the startup line's source attribution). Pure and
/// env-independent, so the precedence rules are unit-testable.
struct ResolvedLogLevel {
    level: &'static str,
    invalid: bool,
    configured: bool,
}

/// Resolve the `[server] loglevel` key into a tracing level string. Absent,
/// blank or unrecognized values keep the crate default (`warn` — errors and
/// warnings only; set `debug` while hunting bugs).
fn resolve_log_level(loglevel: Option<&str>) -> ResolvedLogLevel {
    const DEFAULT_LEVEL: &str = "warn";
    let requested = loglevel.map(str::trim).filter(|s| !s.is_empty());
    let Some(level) = requested else {
        return ResolvedLogLevel {
            level: DEFAULT_LEVEL,
            invalid: false,
            configured: false,
        };
    };
    match level.to_ascii_lowercase().as_str() {
        "error" => ResolvedLogLevel {
            level: "error",
            invalid: false,
            configured: true,
        },
        "warn" => ResolvedLogLevel {
            level: "warn",
            invalid: false,
            configured: true,
        },
        "info" => ResolvedLogLevel {
            level: "info",
            invalid: false,
            configured: true,
        },
        "debug" => ResolvedLogLevel {
            level: "debug",
            invalid: false,
            configured: true,
        },
        "trace" => ResolvedLogLevel {
            level: "trace",
            invalid: false,
            configured: true,
        },
        _ => ResolvedLogLevel {
            level: DEFAULT_LEVEL,
            invalid: true,
            configured: true,
        },
    }
}

/// The default per-crate filter for a level: this crate plus the HTTP layer.
fn default_log_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::new(format!(
        "{crate}={level},tower_http={level}",
        crate = env!("CARGO_CRATE_NAME")
    ))
}

/// Initialize `tracing` from the loaded settings.
///
/// Precedence: an explicit, parsable `RUST_LOG` env var wins (so a one-off debug
/// session needs no config edit); else `[server] loglevel`; else the crate
/// default (`warn`). This runs *after* `load_settings`, so anything logged
/// before it (the instance-lock failure, `ensure_config_file`) reports on
/// stderr instead of through a subscriber that does not exist yet.
fn init_tracing(loglevel: Option<&str>) {
    let resolved = resolve_log_level(loglevel);
    // An empty/blank RUST_LOG would filter everything out, so treat it as unset.
    let rust_log_set = std::env::var("RUST_LOG")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let (source, filter) = match tracing_subscriber::EnvFilter::try_from_default_env() {
        // RUST_LOG wins: report it, so the logged filter is the effective one.
        Ok(from_env) if rust_log_set => ("RUST_LOG", from_env),
        // An unparsable RUST_LOG must not silently disable all logging.
        _ if rust_log_set => (
            if resolved.configured {
                "[server] loglevel (RUST_LOG was invalid)"
            } else {
                "default (RUST_LOG was invalid)"
            },
            default_log_filter(resolved.level),
        ),
        _ => (
            if resolved.configured {
                "[server] loglevel"
            } else {
                "default"
            },
            default_log_filter(resolved.level),
        ),
    };
    // The startup line goes to stderr *before* the subscriber is installed, so
    // it is never subject to the filter it is describing: at `warn` (the crate
    // default, and the fallback for an unknown `loglevel`) a `tracing::info!`
    // startup line is silently dropped, leaving the operator with no record of
    // what verbosity the server actually started with. `ensure_config_file`
    // already uses this pattern for the same reason.
    if resolved.invalid {
        eprintln!(
            "WARN: unknown server.ini loglevel {:?}; using {}",
            loglevel.unwrap_or(""),
            resolved.level
        );
    }
    eprintln!(
        "INFO: Log level: {} (from {source}; set [server] loglevel or RUST_LOG to change)",
        resolved.level
    );
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

#[tokio::main]
async fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("l337-audio-server {}", env!("L337_VERSION"));
        println!();
        println!("Usage: l337-audio-server [OPTIONS]");
        println!();
        println!("Options:");
        println!("  --dummy          Run without audio hardware (no soundcard required)");
        println!("  --transport=MODE Set transport: auto, socket, http (default: auto)");
        println!("  --token          Print the path to the persisted auth token file and exit");
        println!("  --version, -v    Print version and exit");
        println!("  --help, -h       Print this help message");
        println!();
        println!("Configuration: /etc/l337-audio-server/server.ini or ./server.ini");
        println!("Docs: https://github.com/tim-projects/l337-audio-server");
        return;
    }

    if std::env::args().any(|a| a == "--token") {
        let path = token_file_path();
        println!("{}", path.display());
        return;
    }

    if std::env::args().any(|a| a == "--version" || a == "-v") {
        println!("l337-audio-server {}", env!("L337_VERSION"));
        return;
    }

    // Ensure a rustls crypto provider is selected (required before building TLS).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Initialize the platform-specific subsystem (runtime dirs, audio env, etc.).
    platform::init();

    // Acquire the single-instance lock before loading config or opening audio.
    // The guard is held for the lifetime of the process; dropping it on exit
    // removes the lock file. `init_tracing` runs after `load_settings`, so this
    // fatal path reports on stderr rather than through (not yet installed) tracing.
    let _instance_guard = match platform::single_instance::InstanceLock::acquire() {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Ensure a config file exists in the official config directory
    // (/etc/l337-audio-server) so a fresh install starts cleanly instead of
    // crashing on a missing [server] section.
    //
    // In systemd deployments the installer already writes this file and
    // ProtectSystem=strict may make /etc read-only at runtime, so failures
    // here are expected and non-fatal. The fallback CWD server.ini and env
    // vars still allow the server to start.
    ensure_config_file();

    // Load configuration. The official location is /etc/l337-audio-server/
    // (systemd ConfigurationDirectory), with CWD ./server.ini as a local/dev
    // fallback. Environment vars (L337__*) win last.
    let mut settings = load_settings().expect("Failed to load config");

    // Logging is configured from the loaded settings (`[server] loglevel`),
    // so the subscriber can only be initialized here — after the config read.
    init_tracing(settings.server.loglevel.as_deref());

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
    let buffer_max_bytes = settings.server.buffer_max_bytes.unwrap_or(24 * 1024 * 1024);
    let default_target_kbps = settings.server.default_target_kbps.unwrap_or(0);
    let engine = if dummy_mode {
        tracing::warn!("Running in DUMMY output mode. No audio will be produced. Testing only.");
        PlayerEngine::new_dummy(storage, buffer_max_bytes, default_target_kbps)
    } else {
        match PlayerEngine::new(storage, buffer_max_bytes, default_target_kbps) {
            Ok(engine) => engine,
            Err(e) => {
                tracing::error!(
                    "Failed to initialize audio device: {}. \
                     Set dummy = true in server.ini to run without audio hardware.",
                    e
                );
                std::process::exit(1);
            }
        }
    };

    let shared_state: AppState = Arc::new(handlers::SendableEngine(Mutex::new(engine)));

    let recovery_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let recovery_state = Arc::clone(&shared_state);
    let recovery_cancel_clone = Arc::clone(&recovery_cancel);
    tokio::spawn(async move {
        use std::time::Duration;
        loop {
            let idle_interval = Duration::from_secs(5);
            let active_interval = Duration::from_millis(500);
            let (sleep_for, should_try_warm_down) = {
                let engine = recovery_state.0.lock().await;
                if matches!(engine.state, crate::api::models::PlayerStateLabel::Stopped) {
                    (idle_interval, true)
                } else {
                    (active_interval, false)
                }
            };
            tokio::time::sleep(sleep_for).await;
            if recovery_cancel_clone.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let mut engine = recovery_state.0.lock().await;
            if should_try_warm_down && engine.try_warm_down(std::time::Instant::now()) {
                tracing::info!("Idle reaper marked engine cold after inactivity");
            }
            if engine.check_backend_error() {
                if let Err(e) = engine.try_recover() {
                    tracing::error!("Backend recovery failed: {}", e);
                }
            }
        }
    });

    // Resolve the auth token: reuse configured value, else load/persist a stable
    // generated token so the client only needs to copy it once.
    let token = match &settings.server.token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => load_or_create_token(),
    };
    let auth_layer = security::AuthLayer::new(token.clone());

    // Spawn SIGHUP config reload: re-read config + env, rotate token if changed.
    #[cfg(unix)]
    {
        let reload_auth_layer = auth_layer.clone();
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
                                let current = reload_auth_layer.current_token();
                                if current != new_token {
                                    tracing::warn!("Rotating auth token");
                                    reload_auth_layer.update_token(new_token);
                                }
                            }
                        }
                    }
                    Err(e) => tracing::error!("Config reload failed: {}", e),
                }
            }
        });
    }

    let challenge_dir = std::env::var("STATE_DIRECTORY")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("l337-audio-server")
        });
    let challenge_state = Arc::new(ChallengeState::new(challenge_dir));
    #[cfg(feature = "qr")]
    if std::env::args().any(|a| a == "--qr" || a == "--print-pairing-qr") {
        if let Err(e) = challenge_state.issue().await {
            eprintln!("failed to issue challenge: {e}");
            std::process::exit(1);
        }
        if let Some(token) = challenge_state.current_token().await {
            let code = qrcode::QrCode::new(token.as_bytes()).unwrap();
            let s = code
                .render::<char>()
                .quiet_zone(true)
                .module_dimensions(2, 1)
                .build();
            println!("{s}");
        }
        return;
    }
    let socket_mode = use_socket;
    let rate_limiter = Arc::new(RateLimiter::new());

    // Build our application with routes
    let mut app = Router::new()
        .route("/health", get(handlers::health))
        .route("/version", get(handlers::version))
        .route("/setup", get(handlers::setup))
        .route("/", get(|| async { "L337 Audio Server" }))
        .route("/player/play", post(handlers::play))
        .route("/player/play/stream", post(handlers::upload_stream))
        .route("/player/pause", post(handlers::pause))
        .route("/player/stop", post(handlers::stop))
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
        .route("/player/target", post(handlers::set_target))
        .route("/player/session/connect", post(handlers::session_connect))
        .route(
            "/player/session/disconnect",
            post(handlers::session_disconnect),
        )
        .route("/player/status", get(handlers::get_status))
        .route("/player/settings", put(handlers::set_settings))
        .route("/auth/challenge", post(handlers::auth_challenge))
        .route("/auth/redeem", post(handlers::auth_redeem));
    #[cfg(feature = "qr")]
    let app = app.route("/auth/challenge/qr", get(handlers::auth_challenge_qr));
    let mut app = app
        .with_state(shared_state)
        .layer(Extension(challenge_state.clone()))
        .layer(Extension(auth_layer.clone()))
        .layer(Extension(socket_mode))
        .layer(Extension(rate_limiter.clone()));

    app = app.layer(auth_layer);
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
                perms.set_mode(socket_mode_from_settings(&settings.server));
                let _ = std::fs::set_permissions(&path, perms);
            }
        }
        tracing::info!(
            "L337 Audio Server listening on Unix socket {}",
            path.display()
        );
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
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
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
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
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
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
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
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await
                    .unwrap();
            }
        }
    }
}

/// Create a default `server.ini` in the official config directory
/// (/etc/l337-audio-server) when none exists, so the server always has a valid
/// configuration on first run instead of panicking on a missing section. When
/// that directory is not present (local/dev runs) it falls back to
/// `server.ini` next to the binary (CWD).
///
/// Runs before `init_tracing` (it must write the file `load_settings` then
/// reads), so its diagnostics go to stderr — the tracing subscriber does not
/// exist yet, and these lines are the only record of the server having written
/// a config to a fixed system path.
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
                    let _ =
                        std::fs::set_permissions(etc_path, std::fs::Permissions::from_mode(0o600));
                }
                eprintln!(
                    "INFO: No server.ini found; created a default at {}",
                    etc_path.display()
                );
            }
            Err(e) => eprintln!(
                "DEBUG: Could not create default server.ini at {}: {}",
                etc_path.display(),
                e
            ),
        }
        return;
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
            eprintln!(
                "INFO: No server.ini found; created a default at {}",
                cwd_path.display()
            );
        }
        Err(e) => eprintln!(
            "DEBUG: Could not create default server.ini at {}: {}",
            cwd_path.display(),
            e
        ),
    }
}

/// Compute the path to the persisted auth token file.
///
/// This mirrors the directory selection in `load_or_create_token()` so that
/// `--token` reports the same path the server would use at runtime.
fn token_file_path() -> PathBuf {
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
    dir.join("server_token.txt")
}

/// Load a previously generated token from the cache dir, or create + persist one.
///
/// Keeps the token stable across restarts when none is configured explicitly.
fn load_or_create_token() -> String {
    let path = token_file_path();
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
    let generated = crate::auth_challenge::generate_token();
    let _ = std::fs::create_dir_all(path.parent().expect("token path has a parent"));
    if let Err(e) = crate::secrets_fs::atomic_write_secret(&path, &generated) {
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

/// Probe whether `addr` can be bound. Returns `true` if the port is available.
fn tcp_port_available(addr: SocketAddr) -> bool {
    std::net::TcpListener::bind(addr).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `[server] loglevel` is the documented, platform-independent way to turn on
    // playback diagnostics without exporting RUST_LOG in a service `run` script
    // (Termux sends the supervised stdout to /dev/null, so RUST_LOG was the only
    // way to get logs before this key existed).
    #[test]
    fn loglevel_absent_keeps_crate_default() {
        // The default is warn (errors + warnings only); debug is opt-in.
        let r = resolve_log_level(None);
        assert_eq!(r.level, "warn");
        assert!(!r.invalid && !r.configured);
        // Whitespace-only / empty means "not set", not "invalid".
        let r = resolve_log_level(Some("  "));
        assert_eq!(r.level, "warn");
        assert!(!r.invalid && !r.configured);
    }

    #[test]
    fn loglevel_known_values_pass_through_case_insensitively() {
        for (input, expected) in [
            ("debug", "debug"),
            ("INFO", "info"),
            (" warn ", "warn"),
            ("Error", "error"),
            ("trace", "trace"),
        ] {
            let r = resolve_log_level(Some(input));
            assert_eq!(
                r.level, expected,
                "loglevel {input:?} must resolve to {expected:?}"
            );
            assert!(!r.invalid && r.configured);
        }
    }

    #[test]
    fn loglevel_unknown_value_warns_and_falls_back() {
        // A typo must not silently kill logging (nor fail the boot): warn + default.
        let r = resolve_log_level(Some("verbose"));
        assert_eq!(r.level, "warn");
        assert!(r.invalid && r.configured);
        // "off" is rejected from the config key on purpose: only RUST_LOG
        // (environment-controlled) may silence the server entirely, so a config
        // edit cannot leave an install with no diagnostics and no record of it.
        let r = resolve_log_level(Some("off"));
        assert_eq!(r.level, "warn");
        assert!(r.invalid, "loglevel=off must be refused from server.ini");
    }
}
