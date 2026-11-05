use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use axum::http::StatusCode;

#[derive(Clone)]
pub struct RateLimiter {
    inner: std::sync::Arc<Mutex<std::collections::HashMap<SocketAddr, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn check(&self, addr: SocketAddr) -> Result<(), StatusCode> {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut map = self.inner.lock().unwrap();
        let entries = map.entry(addr).or_default();
        entries.retain(|t| now.duration_since(*t) < window);
        if entries.len() >= 5 {
            // Prune entries that have fallen out of the window so the map
            // doesn't grow unboundedly over time (OPT-10).
            if entries.is_empty() {
                map.remove(&addr);
            }
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        entries.push(now);
        // Also prune any other idle entries occasionally when the map is large.
        if map.len() > 512 {
            map.retain(|_, v| !v.is_empty() && v.iter().any(|t| now.duration_since(*t) < window));
        }
        Ok(())
    }
}
