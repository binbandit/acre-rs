//! Parsing `git worktree list`.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::runner::run_git;
use crate::util::canonical_or_absolute;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub branch_ref: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub lock_reason: Option<String>,
    pub prunable: bool,
    pub prune_reason: Option<String>,
    pub is_main: bool,
    pub exists: bool,
}

pub fn list_worktrees(cwd: &Path) -> Result<Vec<GitWorktree>> {
    let result = run_git(cwd, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(parse_worktree_porcelain(&result.stdout))
}

pub fn parse_worktree_porcelain(buffer: &[u8]) -> Vec<GitWorktree> {
    #[derive(Default)]
    struct Current {
        path: Option<PathBuf>,
        head: String,
        branch_ref: Option<String>,
        branch: Option<String>,
        detached: bool,
        bare: bool,
        locked: bool,
        lock_reason: Option<String>,
        prunable: bool,
        prune_reason: Option<String>,
    }

    fn finish(rows: &mut Vec<GitWorktree>, current: &mut Current) {
        let Some(path) = current.path.take() else {
            return;
        };
        rows.push(GitWorktree {
            path,
            head: std::mem::take(&mut current.head),
            branch: current.branch.take(),
            branch_ref: current.branch_ref.take(),
            detached: current.detached,
            bare: current.bare,
            locked: current.locked,
            lock_reason: current.lock_reason.take(),
            prunable: current.prunable,
            prune_reason: current.prune_reason.take(),
            // Git always lists the main worktree first.
            is_main: rows.is_empty(),
            exists: true,
        });
        *current = Current::default();
    }

    let mut rows = Vec::new();
    let mut current = Current::default();
    for raw in buffer.split(|byte| *byte == 0) {
        // Records are separated by a blank line, which lands as a newline glued to the next key.
        let raw = raw.strip_prefix(b"\n").unwrap_or(raw);
        if raw.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(raw);
        let (key, value) = text.split_once(' ').unwrap_or((&text, ""));
        match key {
            "worktree" => {
                finish(&mut rows, &mut current);
                current.path = Some(PathBuf::from(value));
            }
            "HEAD" => current.head = value.to_owned(),
            "branch" => {
                current.branch_ref = Some(value.to_owned());
                current.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).to_owned());
            }
            "detached" => current.detached = true,
            "bare" => current.bare = true,
            "locked" => {
                current.locked = true;
                if !value.is_empty() {
                    current.lock_reason = Some(value.to_owned());
                }
            }
            "prunable" => {
                current.prunable = true;
                if !value.is_empty() {
                    current.prune_reason = Some(value.to_owned());
                }
            }
            _ => {}
        }
    }
    finish(&mut rows, &mut current);
    rows
}

pub fn with_existence(mut worktrees: Vec<GitWorktree>) -> Vec<GitWorktree> {
    for worktree in &mut worktrees {
        worktree.exists = worktree.path.is_dir();
    }
    worktrees
}

pub fn find_current_worktree(worktrees: &[GitWorktree], cwd: &Path) -> Option<GitWorktree> {
    let cwd = canonical_or_absolute(cwd);
    let mut candidates = worktrees.to_vec();
    // Longest path first, so a nested worktree wins over the one that contains it.
    candidates.sort_by_key(|worktree| std::cmp::Reverse(worktree.path.as_os_str().len()));
    candidates.into_iter().find(|worktree| {
        cwd == canonical_or_absolute(&worktree.path) || cwd.starts_with(canonical_or_absolute(&worktree.path))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_delimited_worktrees() {
        let input = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\nworktree /repo-feature\0HEAD def\0detached\0locked busy\0";
        let rows = parse_worktree_porcelain(input);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].branch.as_deref(), Some("main"));
        assert!(rows[0].is_main);
        assert!(rows[1].detached);
        assert!(rows[1].locked);
    }
}
