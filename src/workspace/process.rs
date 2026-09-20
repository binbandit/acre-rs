//! Finds processes whose working directory sits inside a workspace, as evidence it is in use.

use std::path::{Path, PathBuf};

use crate::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessUse {
    pub pid: u32,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

pub fn find_processes_using_path(target: &Path, ignored_pids: &[u32]) -> Result<Vec<ProcessUse>> {
    #[cfg(target_os = "linux")]
    {
        find_linux_processes(target, ignored_pids)
    }
    #[cfg(target_os = "macos")]
    {
        find_macos_processes(target, ignored_pids)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (target, ignored_pids);
        // These platforms have no cwd scanner. Lease checks still apply; this limitation
        // is documented separately from failures of an available scanner.
        Ok(Vec::new())
    }
}

#[cfg(target_os = "linux")]
fn find_linux_processes(target: &Path, ignored_pids: &[u32]) -> Result<Vec<ProcessUse>> {
    use std::collections::BTreeSet;
    use std::fs;

    use crate::util::is_inside;

    let mut ignored: BTreeSet<u32> = ignored_pids.iter().copied().collect();
    ignored.insert(std::process::id());
    let mut result = Vec::new();
    let entries = fs::read_dir("/proc")?;
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
        // Unreadable cwd means another user's process; we can't know, so we don't count it.
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
    Ok(result)
}

#[cfg(target_os = "macos")]
fn find_macos_processes(target: &Path, ignored_pids: &[u32]) -> Result<Vec<ProcessUse>> {
    use std::collections::BTreeSet;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    use crate::git::runner::{RunOptions, run_process};
    use crate::util::is_inside;

    // lsof inherits Acre's working directory, which is usually the workspace being
    // assessed, so run it from the filesystem root to keep it out of its own report.
    let result = run_process(
        "lsof",
        &["-a", "-d", "cwd", "-F", "pcn"],
        RunOptions {
            cwd: Some(PathBuf::from("/")),
            timeout: Some(std::time::Duration::from_secs(10)),
            accepted_statuses: &[0, 1],
            ..RunOptions::default()
        },
    )?;
    if result.status != 0 && !result.stderr.is_empty() {
        return Err(crate::error::AcreError::new(
            "ACRE_PROCESS_CHECK_FAILED",
            "Could not establish whether the workspace is in use.",
            crate::error::exit::REFUSED,
        ));
    }
    let mut ignored: BTreeSet<u32> = ignored_pids.iter().copied().collect();
    ignored.insert(std::process::id());
    let mut rows = Vec::new();
    // lsof uses caret notation for control bytes without C escapes. It does not
    // distinguish those from literal carets, so both interpretations must block return.
    let mut control_target = Vec::new();
    for &byte in crate::util::canonical_or_absolute(target).as_os_str().as_bytes() {
        if matches!(byte, 0..=7 | 11 | 14..=31) {
            control_target.extend_from_slice(&[b'^', byte + b'@']);
        } else {
            control_target.push(byte);
        }
    }
    // -F output is one field per line: p, then c, then n, per process.
    let mut pid = None;
    let mut command = String::new();
    for line in String::from_utf8_lossy(&result.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix('c') {
            command = value.to_owned();
        } else if let Some(value) = line.strip_prefix('n') {
            let Some(pid) = pid else { continue };
            if ignored.contains(&pid) {
                continue;
            }
            let name = decode_lsof_name(value.as_bytes());
            let cwd = PathBuf::from(std::ffi::OsString::from_vec(name.clone()));
            let resolved = is_inside(target, &cwd);
            let control_match = name == control_target
                || name
                    .strip_prefix(control_target.as_slice())
                    .is_some_and(|rest| rest.starts_with(b"/"));
            if resolved || control_match {
                rows.push(ProcessUse {
                    pid,
                    command: if command.is_empty() {
                        format!("pid {pid}")
                    } else {
                        command.clone()
                    },
                    // Caret-escaped names are ambiguous; do not invent an exact cwd.
                    cwd: resolved.then_some(cwd),
                });
            }
        }
    }
    rows.sort_by_key(|process| process.pid);
    Ok(rows)
}

#[cfg(target_os = "macos")]
fn decode_lsof_name(value: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] == b'\\' {
            let escape = match value.get(index + 1) {
                Some(b'\\') => Some((b'\\', 2)),
                Some(b'b') => Some((8, 2)),
                Some(b'f') => Some((12, 2)),
                Some(b'n') => Some((b'\n', 2)),
                Some(b'r') => Some((b'\r', 2)),
                Some(b't') => Some((b'\t', 2)),
                Some(b'x') => value.get(index + 2..index + 4).and_then(|hex| {
                    let high = char::from(hex[0]).to_digit(16)?;
                    let low = char::from(hex[1]).to_digit(16)?;
                    Some(((high * 16 + low) as u8, 4))
                }),
                _ => None,
            };
            if let Some((byte, consumed)) = escape {
                decoded.push(byte);
                index += consumed;
                continue;
            }
        }
        decoded.push(value[index]);
        index += 1;
    }
    decoded
}
