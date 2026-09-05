use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::warn;

#[derive(Debug)]
pub enum ChallengeError {
    Io(std::io::Error),
    Expired,
    Missing,
    Mismatch,
}

impl std::fmt::Display for ChallengeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChallengeError::Io(e) => write!(f, "io error: {}", e),
            ChallengeError::Expired => write!(f, "challenge expired"),
            ChallengeError::Missing => write!(f, "challenge missing"),
            ChallengeError::Mismatch => write!(f, "challenge mismatch"),
        }
    }
}

impl std::error::Error for ChallengeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ChallengeError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ChallengeError {
    fn from(e: std::io::Error) -> Self {
        ChallengeError::Io(e)
    }
}

pub const CHALLENGE_TTL: Duration = Duration::from_secs(600);

pub struct ChallengeState {
    inner: Arc<Mutex<ChallengeInner>>,
}

struct ChallengeInner {
    path: PathBuf,
}

impl ChallengeState {
    pub fn new(config_dir: PathBuf) -> Self {
        let path = config_dir.join("challenge-token.txt");
        Self {
            inner: Arc::new(Mutex::new(ChallengeInner { path })),
        }
    }

    pub async fn issue(&self) -> Result<(), ChallengeError> {
        let inner = self.inner.lock().await;
        let token = generate_token();
        crate::secrets_fs::atomic_write_secret(&inner.path, &token)?;
        warn!("Issued new auth challenge token at {}", inner.path.display());
        Ok(())
    }

    pub async fn verify_and_consume(&self, presented: &str) -> Result<String, ChallengeError> {
        let inner = self.inner.lock().await;

        if !inner.path.exists() {
            return Err(ChallengeError::Missing);
        }

        let metadata = std::fs::metadata(&inner.path)?;
        let modified = metadata.modified()?;
        let elapsed = modified.elapsed().unwrap_or(Duration::from_secs(0));
        if elapsed > CHALLENGE_TTL {
            let _ = std::fs::remove_file(&inner.path);
            return Err(ChallengeError::Expired);
        }

        let stored = std::fs::read_to_string(&inner.path)?;
        let stored = stored.trim();

        if !constant_time_eq(stored, presented) {
            return Err(ChallengeError::Mismatch);
        }

        let token = stored.to_string();
        let server_token_path = inner.path.with_file_name("server_token.txt");

        drop(inner);

        crate::secrets_fs::atomic_write_secret(&server_token_path, &token)?;
        let _ = std::fs::remove_file(&self.inner.lock().await.path);

        Ok(token)
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut result = 0u8;
    for i in 0..a_bytes.len() {
        result |= a_bytes[i] ^ b_bytes[i];
    }
    result == 0
}

pub fn generate_token() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let raw: String = (0..20)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect();
    raw.chars()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 4 == 0 {
                Some('-')
            } else {
                None
            }
            .into_iter()
            .chain(std::iter::once(c))
        })
        .collect()
}
