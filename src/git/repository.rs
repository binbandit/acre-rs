//! Repository discovery: top level, common dir, remotes, default branch, and registered worktrees.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AcreError, Result, exit};
use crate::git::runner::{RunOptions, run_git_with, run_process};
use crate::git::worktrees::{
    GitWorktree, find_current_worktree, list_worktrees, parse_worktree_porcelain, with_existence,
};
use crate::provider::remote::parse_hosted_remote;
use crate::util::{canonical_or_absolute, short_hash};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub top_level: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub remote: Option<String>,
    pub remote_url: Option<String>,
    pub default_branch: Option<String>,
    pub current_worktree: Option<GitWorktree>,
    pub worktrees: Vec<GitWorktree>,
}

impl Repository {
    /// The main worktree's directory, which holds the trusted copies of local files.
    pub fn primary_path(&self) -> &Path {
        self.worktrees
            .first()
            .map(|worktree| worktree.path.as_path())
            .unwrap_or(self.top_level.as_path())
    }

    /// The registered worktree at `path`, compared after canonicalization.
    pub fn worktree_at(&self, path: &Path) -> Option<&GitWorktree> {
        let path = canonical_or_absolute(path);
        self.worktrees
            .iter()
            .find(|worktree| canonical_or_absolute(&worktree.path) == path)
    }
}

pub fn discover_repository(cwd: &Path) -> Result<Repository> {
    let result = run_git_with(
        cwd,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--is-inside-work-tree",
        ],
        RunOptions {
            accepted_statuses: &[0, 128],
            ..RunOptions::default()
        },
    )?;
    if result.status != 0 {
        return Err(AcreError::new(
            "ACRE_NOT_IN_REPOSITORY",
            "This directory is not inside a Git worktree.",
            exit::ENVIRONMENT,
        ));
    }
    let text = String::from_utf8_lossy(&result.stdout);
    let lines: Vec<&str> = text.trim().lines().collect();
    if lines.len() < 4 || lines[3] != "true" {
        return Err(AcreError::new(
            "ACRE_NOT_IN_REPOSITORY",
            "Acre needs a non-bare Git working tree.",
            exit::ENVIRONMENT,
        )
        .with_details(serde_json::json!({ "cwd": cwd, "output": lines })));
    }
    let top_level = PathBuf::from(lines[0]);
    let git_dir = PathBuf::from(lines[1]);
    let common_dir = PathBuf::from(lines[2]);
    let worktrees = with_existence(list_worktrees(&top_level)?);
    let config = read_repository_config(&top_level)?;
    let remote = if config.remote_urls.contains_key("origin") {
        Some("origin".to_owned())
    } else {
        config.remote_urls.keys().next().cloned()
    };
    let remote_url = remote
        .as_ref()
        .and_then(|name| config.remote_urls.get(name))
        .cloned();
    let name = remote_url
        .as_deref()
        .and_then(parse_hosted_remote)
        .map(|remote| remote.repo)
        .or_else(|| {
            worktrees
                .first()
                .and_then(|worktree| worktree.path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repository".to_owned());
    let identity = remote_url
        .as_deref()
        .unwrap_or_else(|| common_dir.to_str().unwrap_or("repository"));
    let remote_head = remote
        .as_deref()
        .and_then(|remote| read_remote_head(&common_dir, remote));
    let default_branch = remote_head.or(config.default_branch).or_else(|| {
        worktrees
            .iter()
            .find(|worktree| worktree.is_main)
            .and_then(|worktree| worktree.branch.clone())
    });
    let current_worktree = find_current_worktree(&worktrees, cwd);

    Ok(Repository {
        id: short_hash(identity, 16),
        name,
        top_level,
        git_dir,
        common_dir,
        remote,
        remote_url,
        default_branch,
        current_worktree,
        worktrees,
    })
}

pub fn discover_repository_from_common_dir(common_dir: &Path) -> Result<Repository> {
    let git_dir = format!("--git-dir={}", common_dir.display());
    let parent = common_dir.parent().unwrap_or_else(|| Path::new("."));
    let result = run_process(
        "git",
        &[&git_dir, "worktree", "list", "--porcelain", "-z"],
        RunOptions {
            cwd: Some(parent.to_path_buf()),
            accepted_statuses: &[0, 128],
            ..RunOptions::default()
        },
    )?;
    if result.status != 0 {
        return Err(AcreError::new(
            "ACRE_REPOSITORY_UNAVAILABLE",
            "The repository is no longer available.",
            exit::ENVIRONMENT,
        ));
    }
    let first = parse_worktree_porcelain(&result.stdout)
        .into_iter()
        .next()
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_REPOSITORY_UNAVAILABLE",
                "The repository has no usable worktree.",
                exit::ENVIRONMENT,
            )
        })?;
    discover_repository(&first.path)
}

#[derive(Default)]
struct RepositoryConfigSnapshot {
    remote_urls: BTreeMap<String, String>,
    default_branch: Option<String>,
}

fn read_repository_config(cwd: &Path) -> Result<RepositoryConfigSnapshot> {
    let result = run_git_with(
        cwd,
        &[
            "config",
            "--null",
            "--get-regexp",
            "^(remote\\..*\\.url|init\\.defaultBranch)$",
        ],
        RunOptions {
            accepted_statuses: &[0, 1],
            ..RunOptions::default()
        },
    )?;
    let mut snapshot = RepositoryConfigSnapshot::default();
    for record in result
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let text = String::from_utf8_lossy(record);
        let Some((key, value)) = text.split_once('\n') else {
            continue;
        };
        if let Some(remote) = key
            .strip_prefix("remote.")
            .and_then(|key| key.strip_suffix(".url"))
        {
            snapshot.remote_urls.insert(remote.to_owned(), value.to_owned());
        } else if key == "init.defaultBranch" {
            snapshot.default_branch = Some(value.to_owned());
        }
    }
    Ok(snapshot)
}

fn read_remote_head(common_dir: &Path, remote: &str) -> Option<String> {
    let path = common_dir.join("refs").join("remotes").join(remote).join("HEAD");
    let text = fs::read_to_string(path).ok()?;
    let prefix = format!("ref: refs/remotes/{remote}/");
    text.trim().strip_prefix(&prefix).map(ToOwned::to_owned)
}
