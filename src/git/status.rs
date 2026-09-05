//! Working tree status and the ignored-path listing.

use std::path::Path;

use crate::error::Result;
use crate::git::runner::{RunOptions, run_git, run_git_with};
use crate::util::sha256;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeStatus {
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub entries: Vec<StatusEntry>,
    pub dirty: bool,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub path: String,
    pub original_path: Option<String>,
    pub index: String,
    pub worktree: String,
    pub kind: StatusEntryKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusEntryKind {
    Changed,
    Untracked,
}

pub fn read_status(cwd: &Path) -> Result<WorkingTreeStatus> {
    let result = run_git(cwd, &["status", "--porcelain=v2", "-z", "--untracked-files=all"])?;
    Ok(parse_status(&result.stdout))
}

pub fn parse_status(buffer: &[u8]) -> WorkingTreeStatus {
    let fields: Vec<String> = buffer
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    let mut entries = Vec::new();
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    let mut conflicted = 0;
    let mut index = 0;

    while index < fields.len() {
        let row = &fields[index];
        // Untracked entries are `? path`, nothing else to parse.
        if let Some(path) = row.strip_prefix("? ") {
            untracked += 1;
            entries.push(StatusEntry {
                path: path.to_owned(),
                original_path: None,
                index: "?".to_owned(),
                worktree: "?".to_owned(),
                kind: StatusEntryKind::Untracked,
            });
            index += 1;
            continue;
        }
        if row.starts_with("1 ") || row.starts_with("2 ") || row.starts_with("u ") {
            let parts: Vec<&str> = row.split(' ').collect();
            // XY: index state then worktree state; `.` means unchanged on that side.
            let xy = parts.get(1).copied().unwrap_or("..");
            let rename = row.starts_with("2 ");
            // Porcelain v2 rows: `1 XY sub mH mI mW hH hI path`, `2 XY sub mH mI mW hH hI Xscore path`
            // (the original path follows as the next NUL-separated field), `u XY sub m1 m2 m3 mW h1 h2 h3 path`.
            let path_index = if row.starts_with("1 ") {
                8
            } else if rename {
                9
            } else {
                10
            };
            // Re-join: the path itself may contain spaces.
            let path = parts.get(path_index..).unwrap_or_default().join(" ");
            let original_path = if rename {
                fields.get(index + 1).cloned()
            } else {
                None
            };
            let mut chars = xy.chars();
            let index_state = chars.next().unwrap_or('.');
            let worktree_state = chars.next().unwrap_or('.');
            if index_state != '.' {
                staged += 1;
            }
            if worktree_state != '.' {
                modified += 1;
            }
            if row.starts_with("u ") || xy.contains('U') {
                conflicted += 1;
            }
            entries.push(StatusEntry {
                path,
                original_path,
                index: index_state.to_string(),
                worktree: worktree_state.to_string(),
                kind: StatusEntryKind::Changed,
            });
            // A rename consumed its original-path field too.
            index += if rename { 2 } else { 1 };
            continue;
        }
        index += 1;
    }

    // The fingerprint lets a resumed `done` prove nothing changed while the shell was moving.
    let mut fingerprint_input = Vec::new();
    for entry in &entries {
        fingerprint_input.extend_from_slice(entry.index.as_bytes());
        fingerprint_input.extend_from_slice(entry.worktree.as_bytes());
        fingerprint_input.push(0);
        fingerprint_input.extend_from_slice(entry.path.as_bytes());
        fingerprint_input.push(0);
        if let Some(original) = &entry.original_path {
            fingerprint_input.extend_from_slice(original.as_bytes());
        }
        fingerprint_input.push(b'\n');
    }
    let fingerprint = sha256(fingerprint_input);
    WorkingTreeStatus {
        staged,
        modified,
        untracked,
        conflicted,
        // Any entry at all, staged or not, is work we must not throw away.
        dirty: !entries.is_empty(),
        entries,
        fingerprint,
    }
}

pub fn in_progress_operation(cwd: &Path) -> Result<Option<String>> {
    let result = run_git(
        cwd,
        &[
            "rev-parse",
            "--git-path",
            "MERGE_HEAD",
            "--git-path",
            "rebase-merge",
            "--git-path",
            "rebase-apply",
            "--git-path",
            "CHERRY_PICK_HEAD",
            "--git-path",
            "REVERT_HEAD",
        ],
    )?;
    // Same order as the --git-path arguments above; a rebase has two marker directories.
    let names = ["merge", "rebase", "rebase", "cherry-pick", "revert"];
    for (path, name) in String::from_utf8_lossy(&result.stdout).lines().zip(names) {
        // rev-parse prints the paths whether or not they exist; presence is the signal.
        if Path::new(path).exists() {
            return Ok(Some(name.to_owned()));
        }
    }
    Ok(None)
}

/// Ignored paths reported by Git, collapsed to their topmost ignored directory.
pub fn list_ignored_entries(cwd: &Path) -> Result<Vec<String>> {
    let result = run_git(
        cwd,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            // Collapse ignored directories to one entry; we don't need every file inside node_modules.
            "--directory",
            "--no-empty-directory",
            "-z",
        ],
    )?;
    let mut entries: Vec<String> = result
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect();
    entries.sort();
    Ok(entries)
}

pub fn is_ignored_path(cwd: &Path, relative: &str) -> Result<bool> {
    let result = run_git_with(
        cwd,
        &["check-ignore", "--quiet", "--", relative],
        RunOptions {
            timeout: Some(std::time::Duration::from_secs(10)),
            accepted_statuses: &[0, 1],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_v2() {
        let input = b"1 M. N... 100644 100644 100644 abc abc src/lib.rs\0? new.txt\0";
        let status = parse_status(input);
        assert_eq!(status.staged, 1);
        assert_eq!(status.untracked, 1);
        assert!(status.dirty);
    }
}
