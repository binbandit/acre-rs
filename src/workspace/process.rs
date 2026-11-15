//! Finds processes whose working directory sits inside a workspace, as evidence it is in use.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessUse {
    pub pid: u32,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

/// Every process other than this one and `ignored_pids` standing in `target` or beneath it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn find_processes_using_path(target: &Path, ignored_pids: &[u32]) -> Result<Vec<ProcessUse>> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    use crate::error::{AcreError, exit};
    use crate::util::canonical_or_absolute;

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    let own = std::process::id();
    // A scan that cannot see even this process proves nothing about anyone else's.
    if system.process(Pid::from_u32(own)).is_none() {
        return Err(AcreError::new(
            "ACRE_PROCESS_CHECK_FAILED",
            "Could not establish whether the workspace is in use.",
            exit::REFUSED,
        ));
    }
    // The kernel reports working directories fully resolved, so resolve the target the same way.
    let target = canonical_or_absolute(target);
    let mut processes: Vec<ProcessUse> = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let pid = pid.as_u32();
            // No working directory means another user's process; we can't know, so we don't count it.
            let cwd = process.cwd()?;
            (pid != own && !ignored_pids.contains(&pid) && cwd.starts_with(&target)).then(|| ProcessUse {
                pid,
                command: process.name().to_string_lossy().into_owned(),
                cwd: Some(cwd.to_path_buf()),
            })
        })
        .collect();
    processes.sort_by_key(|process| process.pid);
    Ok(processes)
}

/// Other platforms have no working-directory scanner. Lease checks still apply there.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn find_processes_using_path(_target: &Path, _ignored_pids: &[u32]) -> Result<Vec<ProcessUse>> {
    Ok(Vec::new())
}
