//! OS-backed exclusive locks, released automatically when the file closes or its process exits.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

use crate::error::{AcreError, Result, exit};
use crate::util::ensure_directory;

pub struct RepositoryLock {
    _file: File,
}

impl RepositoryLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_with_timeout(path, Duration::from_secs(30))
    }

    /// Best-effort bookkeeping must not wait behind workspace preparation.
    pub fn try_acquire(path: &Path) -> Result<Self> {
        Self::acquire_with_timeout(path, Duration::ZERO)
    }

    fn acquire_with_timeout(path: &Path, timeout: Duration) -> Result<Self> {
        if let Some(parent) = path.parent() {
            ensure_directory(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|error| AcreError::io(format!("could not open lock {}", path.display()), error))?;
        let started = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                // Keep the file on disk: unlinking it would let another process lock a different inode.
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(AcreError::new(
                        "ACRE_REPOSITORY_BUSY",
                        "Another Acre operation is already changing this state.",
                        exit::CONFLICT,
                    )
                    .with_details(serde_json::json!({ "lockPath": path })));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(AcreError::io(format!("could not lock {}", path.display()), error));
                }
            }
        }
    }
}

pub fn is_pid_alive(pid: u32) -> bool {
    // pid 0 is the idle process on Windows and never a lease holder anywhere; short-circuit it.
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // ps sees other users' processes, where kill -0 would report EPERM and make a live owner look dead.
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.trim_ascii().is_empty())
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            // tasklist has no exit status worth trusting; look for the pid in its output instead.
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        output.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_liveness_tracks_real_processes() {
        assert!(is_pid_alive(std::process::id()));
        assert!(!is_pid_alive(0));
        assert!(!is_pid_alive(u32::MAX - 1));
    }
}
