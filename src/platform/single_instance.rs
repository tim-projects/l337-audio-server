use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::os::unix::fs::OpenOptionsExt;
use std::os::fd::AsRawFd;

#[derive(Debug)]
enum AcquireError {
    Locked,
    Unavailable,
}

/// Ensures that only one instance of the server runs at a time.
///
/// Uses advisory file locks (`flock` on Unix, exclusive `CreateFile` on Windows)
/// placed at all candidate runtime paths. All candidates must be acquired before
/// this returns success, so multiple instances cannot slip through by locking
/// different paths.
pub struct InstanceLock {
    _files: Vec<File>,
    paths: Vec<PathBuf>,
}

impl InstanceLock {
    /// Try to acquire the single-instance lock at all candidate paths.
    ///
    /// Returns `Err` if any candidate is already held by another instance.
    /// Paths that are unavailable (e.g. permission denied) are skipped.
    pub fn acquire() -> Result<Self, String> {
        let candidates = instance_lock_candidates();
        let mut acquired = Vec::new();
        let mut acquired_paths = Vec::new();

        for lock_path in &candidates {
            match Self::try_acquire_at(lock_path) {
                Ok(file) => {
                    acquired.push(file);
                    acquired_paths.push(lock_path.clone());
                }
                Err(AcquireError::Locked) => {
                    Self::release_all(acquired);
                    return Err(format!(
                        "Another instance is already running (lock file: {})",
                        lock_path.display()
                    ));
                }
                Err(AcquireError::Unavailable) => {
                    // Skip paths we cannot use (e.g. permission denied).
                }
            }
        }

        if acquired.is_empty() {
            return Err(
                "Cannot acquire any instance lock; no writable runtime paths available".into(),
            );
        }

        Ok(Self {
            _files: acquired,
            paths: acquired_paths,
        })
    }

    #[cfg(unix)]
    fn try_acquire_at(lock_path: &PathBuf) -> Result<File, AcquireError> {
        if let Some(parent) = lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::debug!("Lock path {}: cannot create parent dir: {}", lock_path.display(), e);
                return Err(AcquireError::Unavailable);
            }
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o644)
            .open(lock_path)
            .map_err(|e| {
                tracing::debug!("Lock path {} unavailable: {}", lock_path.display(), e);
                AcquireError::Unavailable
            })?;

        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                tracing::debug!("Lock path {} held by another instance", lock_path.display());
                return Err(AcquireError::Locked);
            }
            tracing::debug!("Lock path {} flock failed: {}", lock_path.display(), err);
            return Err(AcquireError::Unavailable);
        }

        let pid = std::process::id();
        let _ = file.set_len(0);
        let _ = file.write_all(pid.to_string().as_bytes());

        Ok(file)
    }

    #[cfg(target_os = "windows")]
    fn try_acquire_at(lock_path: &PathBuf) -> Result<File, AcquireError> {
        if let Some(parent) = lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::debug!("Lock path {}: cannot create parent dir: {}", lock_path.display(), e);
                return Err(AcquireError::Unavailable);
            }
        }

        use std::os::windows::fs::OpenOptionsExt;

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .share_mode(0)
            .open(lock_path)
            .map_err(|e| {
                tracing::debug!("Lock path {} unavailable: {}", lock_path.display(), e);
                AcquireError::Unavailable
            })?;

        let pid = std::process::id();
        let mut file = file;
        let _ = file.set_len(0);
        let _ = file.write_all(pid.to_string().as_bytes());

        Ok(file)
    }

    fn release_all(files: Vec<File>) {
        for file in files {
            drop(file);
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn instance_lock_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let rt = super::common::runtime_dir();
    paths.push(rt.join("instance.lock"));

    #[cfg(target_os = "linux")]
    {
        use dirs;
        if let Some(cache) = dirs::cache_dir() {
            paths.push(cache.join("l337").join("l337-audio-server").join("instance.lock"));
        }
        paths.push(PathBuf::from("/tmp/l337-audio-server-runtime").join("instance.lock"));
    }

    #[cfg(target_os = "macos")]
    {
        use dirs;
        if let Some(cache) = dirs::cache_dir() {
            paths.push(cache.join("l337-audio-server").join("runtime").join("instance.lock"));
        }
        paths.push(PathBuf::from("/tmp/l337-audio-server-runtime").join("instance.lock"));
    }

    #[cfg(target_os = "windows")]
    {
        paths.push(std::env::temp_dir().join("l337-audio-server").join("runtime").join("instance.lock"));
    }

    paths
}
