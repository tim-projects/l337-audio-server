use crate::platform::common::{audio_env, ensure_runtime_dir};

/// Linux-specific platform implementation.
///
/// This module is compiled only for Linux targets and contains:
/// - PipeWire runtime directory setup
/// - Environment variables required for PipeWire
/// - systemd service paths
pub fn init() {
    ensure_runtime_dir();
    for (key, value) in audio_env() {
        unsafe { std::env::set_var(key, value) };
    }
}

/// Verify that PipeWire is available on the system.
pub fn check_pipewire_available() -> bool {
    std::process::Command::new("pipewire")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Verify that WirePlumber is available on the system.
pub fn check_wireplumber_available() -> bool {
    std::process::Command::new("wireplumber")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Start the PipeWire service wrapper script.
pub fn start_pipewire_service() -> Result<(), String> {
    let script = "/opt/l337-audio-server/scripts/start-pipewire.sh";
    if !std::path::Path::new(script).exists() {
        return Err(format!("PipeWire wrapper script not found: {}", script));
    }

    std::process::Command::new(script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to start PipeWire service: {}", e))
}

#[cfg(test)]
mod tests {
    use crate::platform::common::runtime_dir;

    #[test]
    fn test_runtime_dir_is_absolute() {
        let dir = runtime_dir();
        assert!(dir.is_absolute());
    }
}
