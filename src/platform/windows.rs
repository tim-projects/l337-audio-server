use crate::platform::common::{audio_env, runtime_dir};

/// Windows-specific platform implementation.
///
/// This module is compiled only for Windows targets. Currently a placeholder
/// for future Windows Service integration.
pub fn init() {
    let _dir = runtime_dir();
    for (key, value) in audio_env() {
        if std::env::var(key).is_err() {
            unsafe { std::env::set_var(key, value) };
        }
    }
    // TODO: Ensure Windows service data directory exists
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder() {
        assert!(runtime_dir().is_absolute());
    }
}
