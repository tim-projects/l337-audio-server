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
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "arm", target_arch = "x86")))]
        let arch = "unknown";

        Self { os, arch, display_name }
    }
}

/// Audio buffer shared between the engine and the native audio backend callback.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub pcm: Vec<f32>,
    pub read_pos: usize,
    pub channels: u16,
    pub sample_rate: u32,
    pub file_sample_rate: u32,
    pub speed: f32,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, file_sample_rate: u32) -> Self {
        Self {
            pcm: Vec::new(),
            read_pos: 0,
            channels: 0,
            sample_rate,
            file_sample_rate,
            speed: 1.0,
        }
    }
}

/// Runtime directory for the platform's session bus / IPC.
/// On Linux with PipeWire this is `/run/l337-audio-server`.
/// On other platforms it falls back to a cache dir.
pub fn runtime_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/run/l337-audio-server")
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
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    {
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
