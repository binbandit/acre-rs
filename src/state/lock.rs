use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{AcreError, Result, exit};
use crate::util::{ensure_directory, now_iso, random_id, read_json, remove_path, write_json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LockOwner {
    token: String,
    pid: u32,
    created_at: String,
}

pub struct RepositoryLock {
    path: PathBuf,
    token: String,
    released: bool,
}

impl RepositoryLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let timeout = Duration::from_secs(30);
        if let Some(parent) = path.parent() {
            ensure_directory(parent)?;
        }
        let token = random_id();
        let started = Instant::now();
        loop {
            match fs::create_dir(path) {
                Ok(()) => {
                    let owner = LockOwner {
                        token: token.clone(),
                        pid: std::process::id(),
                        created_at: now_iso(),
                    };
                    write_json(&path.join("owner.json"), &owner)?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                        token,
                        released: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = read_json::<LockOwner>(&path.join("owner.json"))?;
                    let abandoned = owner.as_ref().is_some_and(|owner| !is_pid_alive(owner.pid))
                        || (owner.is_none() && lock_is_clearly_abandoned(path));
                    if abandoned {
                        let _ = remove_path(path);
                        continue;
                    }
                    if started.elapsed() >= timeout {
                        return Err(AcreError::new(
                            "ACRE_REPOSITORY_BUSY",
                            "Another Acre operation is already changing this repository.",
                            exit::CONFLICT,
                        )
                        .with_details(serde_json::json!({ "lockPath": path })));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    return Err(AcreError::io(
                        format!("could not acquire {}", path.display()),
                        error,
                    ));
                }
            }
        }
    }

    pub fn release(mut self) -> Result<()> {
        self.release_inner();
        self.released = true;
        Ok(())
    }

    fn release_inner(&self) {
        let owner = read_json::<LockOwner>(&self.path.join("owner.json"))
            .ok()
            .flatten();
        if owner.as_ref().is_some_and(|owner| owner.token == self.token) {
            let _ = remove_path(&self.path);
        }
    }
}

impl Drop for RepositoryLock {
    fn drop(&mut self) {
        if !self.released {
            self.release_inner();
        }
    }
}

pub fn is_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        output.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn lock_is_clearly_abandoned(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_none_or(|age| age > Duration::from_secs(5))
}
