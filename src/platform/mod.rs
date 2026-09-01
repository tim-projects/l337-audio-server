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
pub mod single_instance;

#[cfg(all(target_os = "android", feature = "backend"))]
pub mod android;

#[cfg(all(target_os = "linux", feature = "backend"))]
pub mod linux;

#[cfg(all(target_os = "macos", feature = "backend"))]
pub mod macos;

#[cfg(all(target_os = "windows", feature = "backend"))]
pub mod windows;

/// Initialize the platform-specific subsystem.
///
/// Call this once at startup before any platform-dependent code runs.
pub fn init() {
    #[cfg(all(target_os = "android", feature = "backend"))]
    android::init();

    #[cfg(all(target_os = "linux", feature = "backend"))]
    linux::init();

    #[cfg(all(target_os = "macos", feature = "backend"))]
    macos::init();

    #[cfg(all(target_os = "windows", feature = "backend"))]
    windows::init();
}
