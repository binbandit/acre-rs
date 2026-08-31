use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git::runner::{RunOptions, run_process};
use crate::model::ProcessUse;
use crate::util::is_inside;

pub fn find_processes_using_path(target: &Path, ignored_pids: &[u32]) -> Vec<ProcessUse> {
    #[cfg(target_os = "linux")]
    {
        return find_linux_processes(target, ignored_pids);
    }
    #[cfg(target_os = "macos")]
    {
        return find_macos_processes(target, ignored_pids);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (target, ignored_pids);
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn find_linux_processes(target: &Path, ignored_pids: &[u32]) -> Vec<ProcessUse> {
    let mut ignored: BTreeSet<u32> = ignored_pids.iter().copied().collect();
    ignored.insert(std::process::id());
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return result;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if ignored.contains(&pid) {
            continue;
        }
        let process_root = entry.path();
        let Ok(cwd) = fs::read_link(process_root.join("cwd")) else {
            continue;
        };
        if !is_inside(target, &cwd) {
            continue;
        }
        let command = fs::read_to_string(process_root.join("comm"))
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| format!("pid {pid}"));
        result.push(ProcessUse {
            pid,
            command,
            cwd: Some(cwd),
        });
    }
    result.sort_by_key(|process| process.pid);
    result
}

#[cfg(target_os = "macos")]
fn find_macos_processes(target: &Path, ignored_pids: &[u32]) -> Vec<ProcessUse> {
    let result = run_process(
        "lsof",
        &["-a".into(), "-d".into(), "cwd".into(), "-F".into(), "pcn".into()],
        RunOptions {
            timeout: Some(std::time::Duration::from_secs(10)),
            accepted_statuses: &[0, 1],
            ..RunOptions::default()
        },
    );
    let Ok(result) = result else {
        return Vec::new();
    };
    let mut ignored: BTreeSet<u32> = ignored_pids.iter().copied().collect();
    ignored.insert(std::process::id());
    let mut rows = Vec::new();
    let mut pid = None;
    let mut command = String::new();
    for line in String::from_utf8_lossy(&result.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix('c') {
            command = value.to_owned();
        } else if let Some(value) = line.strip_prefix('n') {
            let Some(pid) = pid else { continue };
            if ignored.contains(&pid) { continue; }
            let cwd = PathBuf::from(value);
            if is_inside(target, &cwd) {
                rows.push(ProcessUse {
                    pid,
                    command: if command.is_empty() { format!("pid {pid}") } else { command.clone() },
                    cwd: Some(cwd),
                });
            }
        }
    }
    rows.sort_by_key(|process| process.pid);
    rows
}
