//! The Git mutations Acre performs: worktree lifecycle, branch binding, fetches.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{AcreError, Result, exit};
use crate::git::repository::Repository;
use crate::git::runner::{ProcessResult, RunOptions, decode_stdout, run_git_with};
use crate::model::{StoredTarget, TargetKind};
use crate::util::ensure_directory;
use crate::workspace::resolve::ResolvedTarget;

// Generous: worktree add on a big repo rewrites the index; fetch gets its own longer limit.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub fn create_detached_worktree(repository: &Repository, target_path: &Path, reference: &str) -> Result<()> {
    if let Some(parent) = target_path.parent() {
        ensure_directory(parent)?;
    }
    if target_path.exists() {
        return Err(path_exists(target_path));
    }
    let path = target_path.display().to_string();
    git(
        &repository.top_level,
        &["worktree", "add", "--detach", &path, reference],
    )?;
    Ok(())
}

pub fn move_worktree(repository: &Repository, from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        ensure_directory(parent)?;
    }
    // Git would refuse anyway; failing early keeps the message ours.
    if to.exists() {
        return Err(path_exists(to));
    }
    let from_text = from.display().to_string();
    let to_text = to.display().to_string();
    let moved = run_git_with(
        &repository.top_level,
        &["worktree", "move", &from_text, &to_text],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    if moved.status == 0 {
        return Ok(());
    }
    // With no source directory left there is nothing to move by hand.
    if !from.exists() {
        return Err(AcreError::new(
            "ACRE_WORKTREE_MOVE_FAILED",
            format!("Acre could not move {}.", from.display()),
            exit::GIT,
        ));
    }
    // Git refused (typically a locked worktree); move the directory and let Git re-link it.
    fs::rename(from, to).map_err(|error| AcreError::io("could not move worktree directory", error))?;
    git(&repository.top_level, &["worktree", "repair", &to_text])?;
    Ok(())
}

pub fn bind_target(repository: &Repository, workspace_path: &Path, target: &ResolvedTarget) -> Result<()> {
    match target.kind {
        TargetKind::NewBranch => {
            let branch = required(target.local_branch.as_deref(), "New branch")?;
            git(workspace_path, &["switch", "--create", branch, &target.oid])?;
        }
        TargetKind::LocalBranch => {
            let branch = required(target.local_branch.as_deref(), "Local branch")?;
            git(workspace_path, &["switch", branch])?;
        }
        TargetKind::RemoteBranch => {
            let branch = required(target.local_branch.as_deref(), "Remote branch")?;
            // Create the local branch at the remote's commit, then track it, like `git switch -c --track`.
            let remote_branch = required(target.remote_branch.as_deref(), "Remote branch")?;
            git(workspace_path, &["switch", "--create", branch, &target.oid])?;
            git(
                workspace_path,
                &["branch", "--set-upstream-to", remote_branch, branch],
            )?;
        }
        // PR heads are checked out detached: there is no local branch to own, and none gets created.
        TargetKind::PullRequest => {
            git(workspace_path, &["switch", "--detach", &target.oid])?;
        }
        TargetKind::Worktree => {
            return Err(AcreError::new(
                "ACRE_TARGET_INVALID",
                "An existing worktree does not need to be bound.",
                exit::INTERNAL,
            ));
        }
    }
    // The ref may have moved between resolving and switching; refuse rather than open the wrong commit.
    verify_bound_head(repository, workspace_path, target)
}

pub fn restore_stored_target(workspace_path: &Path, target: &StoredTarget) -> Result<()> {
    match target.local_branch.as_deref() {
        Some(branch) => git(workspace_path, &["switch", branch])?,
        None => git(workspace_path, &["switch", "--detach", &target.oid])?,
    };
    Ok(())
}

pub fn detach_workspace(workspace_path: &Path) -> Result<()> {
    git(workspace_path, &["switch", "--detach"])?;
    Ok(())
}

pub fn reset_workspace(workspace_path: &Path, oid: &str) -> Result<()> {
    // Tracked files only; ignored caches are cleared separately, by name.
    git(workspace_path, &["reset", "--hard", oid])?;
    git(workspace_path, &["switch", "--detach", oid])?;
    Ok(())
}

pub fn remove_worktree(repository: &Repository, target_path: &Path, force: bool) -> Result<()> {
    let path = target_path.display().to_string();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git(&repository.top_level, &args)?;
    Ok(())
}

pub fn prune_worktrees(repository: &Repository) -> Result<()> {
    git(&repository.top_level, &["worktree", "prune"])?;
    Ok(())
}

pub fn repair_worktrees(repository: &Repository) -> Result<()> {
    git(&repository.top_level, &["worktree", "repair"])?;
    Ok(())
}

pub fn fetch_ref(
    repository: &Repository,
    remote: &str,
    source: &str,
    destination: Option<&str>,
) -> Result<()> {
    let refspec = match destination {
        // Forced: a PR head or default branch may have been rewritten since we last fetched.
        Some(destination) => format!("+{source}:{destination}"),
        None => source.to_owned(),
    };
    run_git_with(
        &repository.top_level,
        &["fetch", "--no-tags", remote, &refspec],
        RunOptions {
            timeout: Some(Duration::from_secs(180)),
            ..RunOptions::default()
        },
    )?;
    Ok(())
}

/// Deletes a branch only while it still points at `expected_oid`; returns whether it did.
pub fn delete_branch_if_expected(repository: &Repository, branch: &str, expected_oid: &str) -> Result<bool> {
    let result = run_git_with(
        &repository.top_level,
        // update-ref with the old value is atomic: it refuses if anyone moved the branch meanwhile.
        &["update-ref", "-d", &format!("refs/heads/{branch}"), expected_oid],
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

fn git(cwd: &Path, args: &[&str]) -> Result<ProcessResult> {
    run_git_with(
        cwd,
        args,
        RunOptions {
            timeout: Some(GIT_TIMEOUT),
            ..RunOptions::default()
        },
    )
}

fn required<'a>(value: Option<&'a str>, kind: &str) -> Result<&'a str> {
    value.ok_or_else(|| {
        AcreError::new(
            "ACRE_TARGET_INVALID",
            format!("{kind} target is incomplete."),
            exit::INTERNAL,
        )
    })
}

fn path_exists(path: &Path) -> AcreError {
    AcreError::new(
        "ACRE_PATH_EXISTS",
        format!("The workspace path already exists: {}", path.display()),
        exit::CONFLICT,
    )
}

fn verify_bound_head(repository: &Repository, workspace_path: &Path, target: &ResolvedTarget) -> Result<()> {
    let actual = decode_stdout(&run_git_with(
        workspace_path,
        &["rev-parse", "HEAD"],
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
