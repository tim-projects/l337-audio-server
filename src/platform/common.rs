use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

#[allow(unused_imports)]
use dirs;

/// Platform metadata detected at compile time.
#[derive(Debug, Clone, Copy)]
pub struct PlatformInfo {
    pub os: &'static str,
    pub arch: &'static str,
    pub display_name: &'static str,
}

impl PlatformInfo {
    pub const fn current() -> Self {
    #[cfg(target_os = "linux")]
    let (os, display_name) = ("linux", "Linux");
    #[cfg(target_os = "android")]
    let (os, display_name) = ("android", "Android");
    #[cfg(target_os = "macos")]
    let (os, display_name) = ("macos", "macOS");
    #[cfg(target_os = "windows")]
    let (os, display_name) = ("windows", "Windows");
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "windows")))]
    let (os, display_name) = ("unknown", "Unknown");

        #[cfg(target_arch = "x86_64")]
        let arch = "x64";
        #[cfg(target_arch = "aarch64")]
        let arch = "arm64";
        #[cfg(target_arch = "arm")]
        let arch = "armv7";
        #[cfg(target_arch = "x86")]
        let arch = "x86";
        #[cfg(target_arch = "riscv64")]
        let arch = "riscv64";
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "arm", target_arch = "x86", target_arch = "riscv64")))]
        let arch = "unknown";

        Self { os, arch, display_name }
    }
}


/// Audio buffer shared between the engine and the native audio backend callback.
#[derive(Debug)]
pub struct AudioBuffer {
    pub pcm: Vec<f32>,
    pub read_pos: usize,
    /// Interleaved samples already reclaimed (drained out of `pcm`) since the
    /// buffer was last (re)filled. `reclaim_consumed` drops the consumed prefix
    /// and resets `read_pos`, so the playhead position is
    /// `consumed_total + read_pos`, never `read_pos` alone — otherwise every
    /// reclaim checkpoint rewinds the reported timeline to the seek base.
    pub consumed_total: usize,
    pub channels: u16,
    pub sample_rate: u32,
    pub file_sample_rate: u32,
    pub speed: f32,
    pub pitch: f32,
    pub     backend_error: std::sync::atomic::AtomicBool,
    /// Set once when the unconsumed window is entered over-window, so writers
    /// log the (non-fatal) diagnostic once per transition, not per sample write.
    over_logged: std::sync::atomic::AtomicBool,
    /// Backend cap in bytes of the *unconsumed rebuffer window* (G4) — a few
    /// seconds of decoded PCM, not a whole track. Backends drain/reclaim consumed
    /// samples as playback advances, so memory stays independent of track length.
    pub max_bytes: usize,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, file_sample_rate: u32) -> Self {
        Self {
            pcm: Vec::new(),
            read_pos: 0,
            consumed_total: 0,
            channels: 0,
            sample_rate,
            file_sample_rate,
            speed: 1.0,
            pitch: 1.0,
            backend_error: std::sync::atomic::AtomicBool::new(false),
            over_logged: std::sync::atomic::AtomicBool::new(false),
            max_bytes: 24 * 1024 * 1024,
        }
    }

    /// True only on the transition into the over-window state (resets on exit),
    /// so a writer logs the non-fatal "over rebuffer window" diagnostic once per
    /// entry instead of on every sample write.
    pub fn enters_over_window(&self) -> bool {
        if self.exceeds_window() {
            !self.over_logged.swap(true, std::sync::atomic::Ordering::Relaxed)
        } else {
            self.over_logged.store(false, std::sync::atomic::Ordering::Relaxed);
            false
        }
    }

    /// Bytes of decoded PCM buffered but not yet consumed. G4 caps *this*
    /// (the rebuffer window), not the whole track — see `AudioBuffer::max_bytes`.
    pub fn unconsumed_bytes(&self) -> usize {
        self.pcm.len().saturating_sub(self.read_pos) * 4
    }

    /// Whether the unconsumed window exceeds the cap. Stream producers throttle
    /// on this so memory stays independent of track length.
    pub fn exceeds_window(&self) -> bool {
        self.unconsumed_bytes() > self.max_bytes
    }

    /// Reclaim already-consumed samples so the Vec shrinks as playback advances
    /// (G4). Call from backend writers at a checkpoint; a no-op once empty.
    pub fn reclaim_consumed(&mut self) {
        let rp = self.read_pos.min(self.pcm.len());
        if rp > 0 {
            self.consumed_total = self.consumed_total.saturating_add(rp);
            self.pcm.drain(0..rp);
            self.read_pos = 0;
            // Bound *capacity* too, not just length: after draining a large
            // consumed prefix (e.g. a whole-track decode on the file path) shrink
            // so memory tracks the window, not the original track length. Gated so
            // the normal streaming path (capacity ~= window) never reallocs.
            let window = self.max_bytes / 4 + (1 << 20);
            if self.pcm.capacity() > window {
                self.pcm.shrink_to_fit();
            }
        }
    }
}

/// Runtime directory for the platform's session bus / IPC.
/// On Linux, honours XDG_RUNTIME_DIR when set (systemd user services) and falls
/// back to /run/l337-audio-server for traditional runs.
/// On other platforms it falls back to a cache dir.
pub fn runtime_dir() -> PathBuf {
    // Escape hatch for isolated instances (tests, side-by-side installs): when
    // set, every runtime path — socket, instance lock and their shared
    // fallbacks — moves inside this directory instead of the platform default,
    // so two instances with different values never collide. Unset in normal
    // operation, where the platform defaults (including the deliberate /tmp
    // fallback for cross-TMPDIR detection) apply.
    if let Some(dir) = std::env::var_os("L337_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && *p != PathBuf::from("/"))
            .unwrap_or_else(|| PathBuf::from("/run/l337-audio-server"))
    }
    #[cfg(target_os = "android")]
    {
        std::env::temp_dir().join("l337-audio-server").join("runtime")
    }
    #[cfg(target_os = "macos")]
    {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("l337-audio-server")
            .join("runtime")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::temp_dir().join("l337-audio-server").join("runtime")
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from("/tmp/l337-audio-server-runtime")
    }
}

/// Ensure the platform runtime directory exists with correct permissions.
pub fn ensure_runtime_dir() {
    let dir = runtime_dir();
    let created = !dir.exists();
    if created {
        let _ = std::fs::create_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    if created {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&dir) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&dir, perms);
        }
    }
}

pub trait AudioOutputStream: Send + Sync {
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn stop(&mut self);
}

pub trait AudioBackend: Send + Sync {
    fn start_stream(
        &self,
        name: &str,
        sample_rate: u32,
        channels: u16,
        audio_buffer: Arc<Mutex<AudioBuffer>>,
        volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String>;
}

pub struct NoopAudioBackend;
pub struct NoopAudioOutputStream;

impl AudioBackend for NoopAudioBackend {
    fn start_stream(
        &self,
        _name: &str,
        _sample_rate: u32,
        _channels: u16,
        _audio_buffer: Arc<Mutex<AudioBuffer>>,
        _volume: Arc<Mutex<f32>>,
    ) -> Result<Box<dyn AudioOutputStream>, String> {
        Ok(Box::new(NoopAudioOutputStream))
    }
}

impl AudioOutputStream for NoopAudioOutputStream {
    fn play(&mut self) -> Result<(), String> { Ok(()) }
    fn pause(&mut self) -> Result<(), String> { Ok(()) }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_info() {
        let info = PlatformInfo::current();
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        assert!(!info.display_name.is_empty());
    }

    #[test]
    fn test_runtime_dir() {
        let dir = runtime_dir();
        assert!(dir.is_absolute());
    }
}
