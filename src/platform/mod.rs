/// Platform-specific code for L337 Audio Server.
///
/// This module isolates OS-dependent behavior so the rest of the codebase
/// stays platform-agnostic. The correct backend is selected at compile time
/// via `#[cfg(target_os = "...")]`.
///
/// Layout:
/// - `common`  — shared types + helpers (always compiled)
/// - `linux`   — Linux / PipeWire / systemd
/// - `macos`   — macOS / launchd (placeholder)
/// - `windows` — Windows / service (placeholder)

pub mod common;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

/// Initialize the platform-specific subsystem.
///
/// Call this once at startup before any platform-dependent code runs.
pub fn init() {
    #[cfg(target_os = "linux")]
    linux::init();

    #[cfg(target_os = "macos")]
    macos::init();

    #[cfg(target_os = "windows")]
    windows::init();
}
