use crate::platform::common::{audio_env, runtime_dir};

/// macOS-specific platform implementation.
///
/// This module is compiled only for macOS targets. Currently a placeholder
/// for future launchd integration.
pub fn init() {
    let _dir = runtime_dir();
    for (key, value) in audio_env() {
        unsafe { std::env::set_var(key, value) };
    }
    // TODO: Ensure launchd plist directory exists
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        assert!(runtime_dir().is_absolute());
    }
}
