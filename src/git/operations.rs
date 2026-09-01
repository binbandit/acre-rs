use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{AcreError, Result, exit, fail};
use crate::git::runner::{RunOptions, decode_stdout, run_git_with};
use crate::model::{Repository, ResolvedTarget, StoredTarget, TargetKind};
use crate::util::{ensure_directory, remove_path};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub fn create_detached_worktree(repository: &Repository, target_path: &Path, reference: &str) -> Result<()> {
    if let Some(parent) = target_path.parent() {
        ensure_directory(parent)?;
    }
    if target_path.exists() {
        return Err(AcreError::new(
            "ACRE_PATH_EXISTS",
            format!("The workspace path already exists: {}", target_path.display()),
            exit::CONFLICT,
        ));
    }
    run_git_with(
        &repository.top_level,
        vec![
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            target_path.display().to_string(),
            reference.into(),
        ],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn move_worktree(repository: &Repository, from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        ensure_directory(parent)?;
    }
    if to.exists() {
        return Err(AcreError::new(
            "ACRE_PATH_EXISTS",
            format!("The workspace path already exists: {}", to.display()),
            exit::CONFLICT,
        ));
    }
    let moved = run_git_with(
        &repository.top_level,
        vec![
            "worktree".into(),
            "move".into(),
            from.display().to_string(),
            to.display().to_string(),
        ],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    if moved.status == 0 {
        return Ok(());
    }
    if !from.exists() {
        return fail(
            "ACRE_WORKTREE_MOVE_FAILED",
            format!("Acre could not move {}.", from.display()),
            exit::GIT,
        );
    }
    fs::rename(from, to).map_err(|error| AcreError::io("could not move worktree directory", error))?;
    run_git_with(
        &repository.top_level,
        vec!["worktree".into(), "repair".into(), to.display().to_string()],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn bind_target(repository: &Repository, workspace_path: &Path, target: &ResolvedTarget) -> Result<()> {
    match target.kind {
        TargetKind::NewBranch => {
            let branch = target.local_branch.as_deref().ok_or_else(|| {
                AcreError::new(
                    "ACRE_TARGET_INVALID",
                    "New branch target is incomplete.",
                    exit::INTERNAL,
                )
            })?;
            git_switch(workspace_path, &["switch", "--create", branch, &target.oid])?;
        }
        TargetKind::LocalBranch => {
            let branch = target.local_branch.as_deref().ok_or_else(|| {
                AcreError::new(
                    "ACRE_TARGET_INVALID",
                    "Local branch target is incomplete.",
                    exit::INTERNAL,
                )
            })?;
            git_switch(workspace_path, &["switch", branch])?;
        }
        TargetKind::RemoteBranch => {
            let branch = target.local_branch.as_deref().ok_or_else(|| {
                AcreError::new(
                    "ACRE_TARGET_INVALID",
                    "Remote branch target is incomplete.",
                    exit::INTERNAL,
                )
            })?;
            let remote_branch = target.remote_branch.as_deref().ok_or_else(|| {
                AcreError::new(
                    "ACRE_TARGET_INVALID",
                    "Remote branch target is incomplete.",
                    exit::INTERNAL,
                )
            })?;
            git_switch(workspace_path, &["switch", "--create", branch, &target.oid])?;
            git_switch(
                workspace_path,
                &["branch", "--set-upstream-to", remote_branch, branch],
            )?;
        }
        TargetKind::PullRequest => git_switch(workspace_path, &["switch", "--detach", &target.oid])?,
        TargetKind::Worktree => {
            return fail(
                "ACRE_TARGET_INVALID",
                "An existing worktree does not need to be bound.",
                exit::INTERNAL,
            );
        }
    }
    verify_bound_head(repository, workspace_path, target)
}

pub fn restore_stored_target(workspace_path: &Path, target: &StoredTarget) -> Result<()> {
    if let Some(branch) = target.local_branch.as_deref() {
        git_switch(workspace_path, &["switch", branch])
    } else {
        git_switch(workspace_path, &["switch", "--detach", &target.oid])
    }
}

pub fn detach_workspace(workspace_path: &Path) -> Result<()> {
    git_switch(workspace_path, &["switch", "--detach"])
}

pub fn reset_workspace(workspace_path: &Path, oid: &str) -> Result<()> {
    git_switch(workspace_path, &["reset", "--hard", oid])?;
    git_switch(workspace_path, &["switch", "--detach", oid])
}

pub fn remove_worktree(repository: &Repository, target_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree".into(), "remove".into()];
    if force {
        args.push("--force".into());
    }
    args.push(target_path.display().to_string());
    run_git_with(
        &repository.top_level,
        args,
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn prune_worktrees(repository: &Repository) -> Result<()> {
    run_git_with(
        &repository.top_level,
        vec!["worktree".into(), "prune".into()],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn repair_worktrees(repository: &Repository) -> Result<()> {
    run_git_with(
        &repository.top_level,
        vec!["worktree".into(), "repair".into()],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn unlock_worktree(repository: &Repository, target_path: &Path) -> Result<()> {
    run_git_with(
        &repository.top_level,
        vec![
            "worktree".into(),
            "unlock".into(),
            target_path.display().to_string(),
        ],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn fetch_ref(
    repository: &Repository,
    remote: &str,
    source: &str,
    destination: Option<&str>,
) -> Result<()> {
    let refspec = destination.map_or_else(
        || source.to_owned(),
        |destination| format!("+{source}:{destination}"),
    );
    run_git_with(
        &repository.top_level,
        vec!["fetch".into(), "--no-tags".into(), remote.into(), refspec],
        RunOptions {
            timeout: Some(Duration::from_secs(180)),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

pub fn delete_path_if_unregistered(path: &Path) -> Result<()> {
    remove_path(path)
}

pub fn delete_branch_if_expected(repository: &Repository, branch: &str, expected_oid: &str) -> Result<bool> {
    let result = run_git_with(
        &repository.top_level,
        vec![
            "update-ref".into(),
            "-d".into(),
            format!("refs/heads/{branch}"),
            expected_oid.into(),
        ],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

fn git_switch(cwd: &Path, args: &[&str]) -> Result<()> {
    run_git_with(
        cwd,
        args.iter().map(|value| (*value).to_owned()).collect(),
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

fn verify_bound_head(repository: &Repository, workspace_path: &Path, target: &ResolvedTarget) -> Result<()> {
    let actual = decode_stdout(&run_git_with(
        workspace_path,
        vec!["rev-parse".into(), "HEAD".into()],
        RunOptions {
            timeout: Some(Duration::from_secs(30)),
            ..RunOptions::default()
        },
    )?);
    if actual == target.oid {
        return Ok(());
    }
    Err(AcreError::new(
        "ACRE_TARGET_MOVED",
        format!(
            "{} changed while Acre was opening it. Acre left the target untouched; run the command again.",
            target.display_name
        ),
        exit::CONFLICT,
    )
    .with_details(serde_json::json!({
        "repository": repository.name,
        "target": target.display_name,
        "expectedOid": target.oid,
        "actualOid": actual,
    })))
}
