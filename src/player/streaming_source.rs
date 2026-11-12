//! Streaming byte source for HTTP-delivered media.
//!
//! Feeds a single, shared, append-only byte source into one long-lived
//! Symphonia `MediaSource`. Reads at the download frontier block until bytes
//! arrive (or true EOF / error / cancel), and `byte_len()` reports the stable
//! `Content-Length` instead of a moving file-size snapshot. This removes the
//! Symphonia MKV "past master element" wedge caused by re-probing a growing
//! file whose frozen `total_len` is always behind the download frontier.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Transfer lifecycle status, stored as a u8 so readers can poll it lock-free.
pub const STATUS_DOWNLOADING: u8 = 0;
pub const STATUS_COMPLETE: u8 = 1;
pub const STATUS_FAILED: u8 = 2;
pub const STATUS_ERROR: u8 = 3;

/// How long a blocking `read` at the download frontier waits for bytes before
/// returning `TimedOut`. A permanently stalled download ends the track rather
/// than freezing the playhead forever.
pub const READ_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared, append-only byte source owned by the download task and cloned into
/// reader handles. All readers share `downloaded`, `total_len`, `status` and
/// `cancel`; each reader tracks its own read cursor.
pub struct StreamingByteSource {
    /// Committed bytes. The downloader appends here; readers slice directly.
    pub data: Arc<Mutex<Vec<u8>>>,
    /// Monotonic count of committed bytes (writer advances as chunks land).
    pub downloaded: AtomicU64,
    /// HTTP `Content-Length`; final, stable logical end. `None` if chunked.
    /// Set once headers resolve; readers wait for it before probing.
    pub total_len: AtomicU64,
    pub total_len_set: AtomicBool,
    /// Transfer lifecycle.
    pub status: AtomicU8,
    /// Failure reason when `Failed`/`Error`.
    pub error: Mutex<Option<String>>,
    /// User stop / track change — must wake all blocked readers.
    pub cancel: AtomicBool,
    /// Wakes blocked readers on append / finish / fail / cancel.
    pub notifier: (Mutex<()>, Condvar),
}

impl StreamingByteSource {
    pub fn new(total_len: Option<u64>) -> Self {
        Self {
            data: Arc::new(Mutex::new(Vec::new())),
            downloaded: AtomicU64::new(0),
            total_len: AtomicU64::new(total_len.unwrap_or(0)),
            total_len_set: AtomicBool::new(total_len.is_some()),
            status: AtomicU8::new(STATUS_DOWNLOADING),
            error: Mutex::new(None),
            cancel: AtomicBool::new(false),
            notifier: (Mutex::new(()), Condvar::new()),
        }
    }

    /// Learn `Content-Length` once headers are resolved, before the body pump.
    pub fn set_total_len(&self, len: Option<u64>) {
        if let Some(n) = len {
            self.total_len.store(n, Ordering::SeqCst);
            self.total_len_set.store(true, Ordering::SeqCst);
            self.notify_all();
        }
    }

    /// Append a chunk of downloaded bytes and wake blocked readers.
    pub fn append(&self, chunk: &[u8]) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        data.extend_from_slice(chunk);
        drop(data);
        let n = chunk.len() as u64;
        self.downloaded.fetch_add(n, Ordering::SeqCst);
        self.notify_all();
    }

    pub fn set_complete(&self) {
        self.status.store(STATUS_COMPLETE, Ordering::SeqCst);
        self.notify_all();
    }

    pub fn set_failed(&self, msg: String) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
        self.status.store(STATUS_FAILED, Ordering::SeqCst);
        self.notify_all();
    }

    /// Cancel the transfer and wake every blocked reader so `spawn_blocking`
    /// decode producers exit promptly on stop/skip.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.notify_all();
    }

    pub fn notify_all(&self) {
        self.notifier.1.notify_all();
    }
}

impl Default for StreamingByteSource {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Probe mode (cherry-picked from koan `PartialFileSource`).
///
/// The demuxer probe and the actual decode have opposite needs:
/// * `Full` — advertises `total_len`, reads block at the frontier. Used for
///   decode of containers that describe themselves from a prefix (MKV/WebM).
/// * `Lengthless` — advertises `None`, end = write head, reads return
///   `UnexpectedEof` at the frontier immediately. Probe of containers that must
///   not hunt for a missing tail.
/// * `LengthlessWholeEnd` — advertises `None`, end = whole file (`total`), so
///   duration isn't reported as 0; reads still don't wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMode {
    Full,
    Lengthless,
    LengthlessWholeEnd,
}

/// One `MediaSource` handle per decoder/probe. Each handle has its own `pos`;
/// all handles share the [`StreamingByteSource`] state.
pub struct StreamingMediaSource {
    source: Arc<StreamingByteSource>,
    pos: u64,
    mode: ProbeMode,
}

impl StreamingMediaSource {
    pub fn new(source: Arc<StreamingByteSource>, mode: ProbeMode) -> Self {
        Self { source, pos: 0, mode }
    }

    /// Block at the download frontier until bytes arrive, the transfer
    /// finishes/fails, or the user cancels. Returns `Ok(())` when new data is
    /// available or the transfer reached a terminal state; `Err(TimedOut)` on
    /// stall; `Err(BrokenPipe)` on cancel.
    fn wait_at_frontier(&self) -> io::Result<()> {
        let (lock, cvar) = &self.source.notifier;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + READ_STALL_TIMEOUT;
        loop {
            if self.source.cancel.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "streaming source canceled"));
            }
            let status = self.source.status.load(Ordering::SeqCst);
            let available = self.source.downloaded.load(Ordering::SeqCst);
            if status != STATUS_DOWNLOADING || available > self.pos {
                return Ok(());
            }
            if self.source.total_len_set.load(Ordering::SeqCst) {
                let total = self.source.total_len.load(Ordering::SeqCst);
                if available >= total && self.pos >= total {
                    return Ok(());
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "streaming read stalled at download frontier",
                ));
            }
            guard = match cvar.wait_timeout(guard, deadline - now) {
                Ok((g, _)) => g,
                Err(_) => lock.lock().unwrap_or_else(|e| e.into_inner()),
            };
        }
    }
}

impl Read for StreamingMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.source.cancel.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "streaming source canceled",
            ));
        }
        loop {
            let available = self.source.downloaded.load(Ordering::SeqCst);
            if available > self.pos {
                let data = self.source.data.lock().unwrap_or_else(|e| e.into_inner());
                let start = self.pos as usize;
                let end = (start + buf.len()).min(data.len()).min(available as usize);
                let count = end.saturating_sub(start);
                if count > 0 {
                    buf[..count].copy_from_slice(&data[start..end]);
                    self.pos += count as u64;
                }
                return Ok(count);
            }
            // At (or past) the write head.
            match self.source.status.load(Ordering::SeqCst) {
                STATUS_FAILED | STATUS_ERROR => {
                    let msg = self
                        .source
                        .error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                        .unwrap_or_else(|| "streaming source failed".into());
                    return Err(io::Error::new(io::ErrorKind::BrokenPipe, msg));
                }
                STATUS_COMPLETE => {
                    if self.pos >= available {
                        return Ok(0); // real EOF
                    }
                    // More data than the downloaded count claims; loop to read it.
                }
                STATUS_DOWNLOADING => {
                    if self.source.total_len_set.load(Ordering::SeqCst) {
                        let total = self.source.total_len.load(Ordering::SeqCst);
                        if available >= total && self.pos >= total {
                            return Ok(0);
                        }
                    }
                    match self.mode {
                        ProbeMode::Full => {
                            self.wait_at_frontier()?;
                            continue; // re-check after wake
                        }
                        ProbeMode::Lengthless | ProbeMode::LengthlessWholeEnd => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "past what has arrived",
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl Seek for StreamingMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(delta) => self.pos as i64 + delta,
            SeekFrom::End(delta) => {
                let end: i64 = match self.mode {
                    ProbeMode::Lengthless => self.source.downloaded.load(Ordering::SeqCst) as i64,
                    ProbeMode::Full | ProbeMode::LengthlessWholeEnd => {
                        if self.source.total_len_set.load(Ordering::SeqCst) {
                            self.source.total_len.load(Ordering::SeqCst) as i64
                        } else {
                            self.source.downloaded.load(Ordering::SeqCst) as i64
                        }
                    }
                };
                end + delta
            }
        };
        let new_pos = new_pos.max(0) as u64;
        self.pos = new_pos;
        Ok(new_pos)
    }
}

impl symphonia::core::io::MediaSource for StreamingMediaSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        match self.mode {
            ProbeMode::Full => {
                if self.source.total_len_set.load(Ordering::SeqCst) {
                    Some(self.source.total_len.load(Ordering::SeqCst))
                } else {
                    None
                }
            }
            ProbeMode::Lengthless => None,
            ProbeMode::LengthlessWholeEnd => None,
        }
    }
}

/// Async body pump: resolve headers, learn `Content-Length`, append chunks to
/// the shared source (and optionally tee to a slot file for cache/persistence),
/// then mark complete or failed.
pub async fn stream_http_to_source(
    http_client: &Arc<reqwest::Client>,
    url: &str,
    source: Arc<StreamingByteSource>,
    cancel: Arc<AtomicBool>,
    tee_path: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let response = http_client
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    if !response.status().is_success() {
        let msg = format!("stream fetch failed ({}): {}", response.status(), url);
        source.set_failed(msg.clone());
        return Err(msg.into());
    }

    source.set_total_len(response.content_length());

    let mut file = match tee_path {
        Some(path) => Some(
            tokio::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)
                .await?,
        ),
        None => None,
    };

    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let chunk = item?;
        source.append(&chunk);
        if let Some(ref mut f) = file {
            f.write_all(&chunk).await?;
        }
    }
    if let Some(mut f) = file {
        f.flush().await?;
    }
    source.set_complete();
    Ok(())
}
