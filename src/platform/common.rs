use std::path::PathBuf;

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
        #[cfg(target_os = "macos")]
        let (os, display_name) = ("macos", "macOS");
        #[cfg(target_os = "windows")]
        let (os, display_name) = ("windows", "Windows");
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
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

/// Runtime directory for the platform's session bus / IPC.
/// On Linux with PipeWire this is `/run/l337-audio-server`.
/// On other platforms it falls back to a cache dir.
pub fn runtime_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/run/l337-audio-server")
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
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from("/tmp/l337-audio-server-runtime")
    }
}

/// Environment variables that must be set before opening an audio stream.
pub fn audio_env() -> Vec<(&'static str, String)> {
    let mut vars = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let rt = runtime_dir();
        vars.push(("XDG_RUNTIME_DIR", rt.display().to_string()));
        vars.push(("PIPEWIRE_RUNTIME_DIR", rt.display().to_string()));
    }
    #[cfg(target_os = "macos")]
    {
        let rt = runtime_dir();
        vars.push(("XDG_RUNTIME_DIR", rt.display().to_string()));
    }
    #[cfg(target_os = "windows")]
    {
        let _ = ();
    }

    vars
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
