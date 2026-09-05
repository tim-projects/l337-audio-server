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
        let mut map = self.inner.lock().unwrap();
        let entries = map.entry(addr).or_default();
        entries.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        if entries.len() >= 5 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        entries.push(now);
        Ok(())
    }
}
