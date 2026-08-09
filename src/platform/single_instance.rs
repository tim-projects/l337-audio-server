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
/// Uses an advisory file lock (`flock` on Unix, exclusive `CreateFile` on Windows)
/// placed in the platform runtime directory. The lock is automatically released
/// by the OS if the process crashes, so stale locks are never a problem.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Try to acquire the single-instance lock.
    ///
    /// Returns `Err` if another instance already holds the lock.
    pub fn acquire() -> Result<Self, String> {
        let candidates = instance_lock_candidates();

        let mut last_err = None;
        let mut locked_path = None;
        for lock_path in candidates {
            match Self::try_acquire_at(&lock_path) {
                Ok(lock) => return Ok(lock),
                Err(AcquireError::Locked) => {
                    locked_path = Some(lock_path);
                }
                Err(AcquireError::Unavailable) => {
                    last_err = Some(lock_path);
                }
            }
        }

        let path = locked_path.or(last_err).unwrap_or_else(|| super::common::runtime_dir().join("instance.lock"));
        Err(format!(
            "Another instance is already running (lock file: {})",
            path.display()
        ))
    }

    #[cfg(unix)]
    fn try_acquire_at(lock_path: &PathBuf) -> Result<Self, AcquireError> {
        let file = OpenOptions::new()
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
        let mut file = file;
        let _ = file.set_len(0);
        let _ = file.write_all(pid.to_string().as_bytes());

        Ok(Self { _file: file, path: lock_path.clone() })
    }

    #[cfg(target_os = "windows")]
    fn try_acquire_at(lock_path: &PathBuf) -> Result<Self, AcquireError> {
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

        Ok(Self { _file: file, path: lock_path.clone() })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
