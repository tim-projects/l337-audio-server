use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use axum::http::StatusCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Auth,
    Play,
    PlayStream,
    Cache,
    Control,
    Seek,
    Session,
    Settings,
    Status,
    Lookup,
    Health,
}

impl Endpoint {
    pub fn from_path(path: &str) -> Self {
        match path {
            "/auth/challenge" | "/auth/redeem" | "/auth/challenge/qr" => Endpoint::Auth,
            "/player/play" => Endpoint::Play,
            "/player/play/stream" => Endpoint::PlayStream,
            "/player/cache/next" | "/player/cache/next/stream"
            | "/player/cache/previous" | "/player/cache/previous/stream" => Endpoint::Cache,
            "/player/seek" => Endpoint::Seek,
            "/player/pause" | "/player/stop" | "/player/next" | "/player/previous"
            | "/player/speed" | "/player/volume" | "/player/target" => Endpoint::Control,
            "/player/session/connect" | "/player/session/disconnect" => Endpoint::Session,
            "/player/settings" | "/player/sinks" | "/player/sink" => Endpoint::Settings,
            "/player/status" => Endpoint::Status,
            "/player/cache/lookup" => Endpoint::Lookup,
            "/health" | "/version" | "/setup" | "/" => Endpoint::Health,
            _ => Endpoint::Control,
        }
    }

    pub fn limit(&self) -> usize {
        match self {
            Endpoint::Auth => 5,
            Endpoint::Play => 30,
            Endpoint::PlayStream => 10,
            Endpoint::Cache => 20,
            Endpoint::Seek => 20,
            Endpoint::Control => 60,
            Endpoint::Session => 10,
            Endpoint::Settings => 10,
            Endpoint::Status => 120,
            Endpoint::Lookup => 30,
            Endpoint::Health => 60,
        }
    }

    pub fn window(&self) -> Duration {
        Duration::from_secs(60)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RateLimitKey {
    endpoint: Endpoint,
    client_id: String,
}

#[derive(Clone)]
pub struct RateLimiter {
    inner: std::sync::Arc<Mutex<std::collections::HashMap<RateLimitKey, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn check_tcp(&self, addr: SocketAddr, endpoint: Endpoint) -> Result<(), StatusCode> {
        let key = RateLimitKey {
            endpoint,
            client_id: addr.to_string(),
        };
        self.check_internal(key, endpoint)
    }

    pub fn check_unix(&self, endpoint: Endpoint) -> Result<(), StatusCode> {
        let key = RateLimitKey {
            endpoint,
            client_id: "unix".to_string(),
        };
        self.check_internal(key, endpoint)
    }

    fn check_internal(&self, key: RateLimitKey, endpoint: Endpoint) -> Result<(), StatusCode> {
        let now = Instant::now();
        let window = endpoint.window();
        let limit = endpoint.limit();

        let mut map = self.inner.lock().unwrap();
        let key_for_remove = key.clone();
        let entries = map.entry(key).or_default();
        entries.retain(|t| now.duration_since(*t) < window);

        if entries.len() >= limit {
            if entries.is_empty() {
                map.remove(&key_for_remove);
            }
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        entries.push(now);

        if map.len() > 512 {
            map.retain(|_, v| !v.is_empty() && v.iter().any(|t| now.duration_since(*t) < window));
        }
        Ok(())
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}