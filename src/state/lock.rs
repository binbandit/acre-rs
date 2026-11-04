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
    if pid == 0 || (cfg!(unix) && pid > i32::MAX as u32) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        if !Path::new("/proc/self").exists() {
            return true;
        }
        Path::new("/proc")
            .join(pid.to_string())
            .try_exists()
            .unwrap_or(true)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // ps sees other users' processes, where kill -0 would report EPERM and make a live owner look dead.
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .map(|output| {
                !(output.status.code() == Some(1)
                    && output.stdout.trim_ascii().is_empty()
                    && output.stderr.trim_ascii().is_empty())
            })
            .unwrap_or(true)
    }
    #[cfg(windows)]
    {
        // tasklist's exit status does not say whether the pid matched, so a successful run is
        // judged by whether a row for exactly this pid came back. A CSV row reads
        // `"image.exe","1234",...`; image names cannot contain a quote. A failed run proves
        // nothing, so the pid stays alive.
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        let pid = pid.to_string();
        output
            .map(|output| {
                !output.status.success()
                    || String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .any(|row| row.split("\",\"").nth(1) == Some(pid.as_str()))
            })
            .unwrap_or(true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_liveness_tracks_real_processes() {
        assert!(is_pid_alive(std::process::id()));
        assert!(!is_pid_alive(0));
        // pid_t is signed, so no Unix process can have this pid.
        #[cfg(unix)]
        assert!(!is_pid_alive(u32::MAX - 1));
        // The open handle keeps Windows from giving this pid to another process before the check.
        let mut exited = std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("start git");
        exited.wait().expect("wait for git");
        assert!(!is_pid_alive(exited.id()), "{}", alive_report(exited.id()));
    }

    /// Windows CI is the only place the tasklist path runs, so its failure says what tasklist said.
    fn alive_report(pid: u32) -> String {
        let mut report = format!("pid {pid} still reads as alive after exiting");
        if cfg!(windows)
            && let Ok(output) = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
        {
            report.push_str(&format!(
                "; tasklist {}: stdout {:?}, stderr {:?}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        report
    }
}
