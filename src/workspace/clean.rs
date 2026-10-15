//! Conservative cleanup: only proven merges and missing Acre-owned worktrees.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{AcreError, Result, exit};
use crate::git::operations::{delete_merged_branch, remove_worktree};
use crate::git::refs::{GitRef, GitRefKind, is_ancestor, list_refs};
use crate::git::repository::{Repository, discover_repository};
use crate::git::status::in_progress_operation;
use crate::model::{AcreConfig, WorkspaceOwnership};
use crate::state::repository::LockedRepository;
use crate::util::{canonical_or_absolute, is_inside};
use crate::workspace::assess::{AssessOptions, assess_workspace};
use crate::workspace::release::return_workspace_locked;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanItem {
    pub action: CleanAction,
    pub description: String,
    pub path: Option<PathBuf>,
    pub branch: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanAction {
    ReturnWorkspace,
    RemoveStaleReference,
    DeleteBranch,
}

impl CleanAction {
    pub fn verb(&self, applied: bool) -> &'static str {
        match (self, applied) {
            (Self::ReturnWorkspace, false) => "return",
            (Self::ReturnWorkspace, true) => "Returned",
            (Self::RemoveStaleReference, false) => "remove",
            (Self::RemoveStaleReference, true) => "Removed",
            (Self::DeleteBranch, false) => "delete",
            (Self::DeleteBranch, true) => "Deleted",
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub base: String,
    pub base_oid: String,
    pub dry_run: bool,
    pub candidates: Vec<CleanItem>,
    pub cleaned: Vec<CleanItem>,
    pub skipped: Vec<CleanItem>,
}

impl CleanReport {
    fn record(&mut self, mut item: CleanItem, reason: Option<String>) {
        item.reason = reason;
        if item.reason.is_some() {
            self.skipped.push(item);
        } else if self.dry_run {
            self.candidates.push(item);
        } else {
            self.cleaned.push(item);
        }
    }
}

pub fn clean_repository(
    config: &AcreConfig,
    repository: &Repository,
    base: Option<&str>,
    apply: bool,
    shell_directory: &Path,
) -> Result<CleanReport> {
    let invocation_dir = repository.invocation_dir.clone();
    let mut locked = LockedRepository::open(config, repository)?;
    let refs = list_refs(&locked.repository.top_level)?;
    let base = cleanup_base(&locked.repository, &refs, base)?;
    let mut report = CleanReport {
        base: base.qualified_name(),
        base_oid: base.oid.clone(),
        dry_run: !apply,
        candidates: Vec::new(),
        cleaned: Vec::new(),
        skipped: Vec::new(),
    };
    let protected = |branch: &str| {
        branch == base.short_name || locked.repository.default_branch.as_deref() == Some(branch)
    };
    let protected_branches: BTreeSet<_> = refs
        .iter()
        .filter(|reference| protected(&reference.short_name))
        .map(|reference| reference.short_name.clone())
        .collect();
    // A preview simulates the releases only for the subsequent branch eligibility check.
    let mut released = BTreeSet::new();
    let options = AssessOptions::default();
    for workspace in locked.state.workspaces.clone() {
        if workspace.ownership != WorkspaceOwnership::Acre || !workspace.path.exists() {
            continue;
        }
        let item = CleanItem {
            action: CleanAction::ReturnWorkspace,
            description: format!("workspace {}", workspace.target.display_name),
            path: Some(workspace.path.clone()),
            branch: workspace.target.local_branch.clone(),
            reason: None,
        };
        let reason = match locked.repository.worktree_at(&workspace.path) {
            None => Some("worktree registration is missing".to_owned()),
            Some(worktree)
                if worktree.is_main
                    || locked.repository.primary_worktree() == Some(worktree)
                    || worktree
                        .branch
                        .as_ref()
                        .is_some_and(|branch| protected_branches.contains(branch)) =>
            {
                Some("primary, default, or base branch workspace".to_owned())
            }
            Some(_)
                if is_inside(&workspace.path, shell_directory)
                    || is_inside(&workspace.path, &invocation_dir) =>
            {
                Some("current workspace; run cleanup from the primary checkout".to_owned())
            }
            Some(worktree) if !is_ancestor(&locked.repository.top_level, &worktree.head, &base.oid)? => {
                Some(format!("not merged into {}", report.base))
            }
            Some(_) => {
                match assess_workspace(&locked.repository, &locked.state, &workspace, config, &options) {
                    Ok(assessment) if assessment.safe => None,
                    Ok(assessment) => Some(assessment.reasons.join("; ")),
                    Err(error) => Some(error.to_string()),
                }
            }
        };
        let reason = if reason.is_none() && apply {
            match return_workspace_locked(config, &mut locked, &workspace.id, &options) {
                Ok(result) if result.returned => {
                    locked.repository = discover_repository(&locked.repository.top_level)?;
                    None
                }
                Ok(result) => Some(result.assessment.reasons.join("; ")),
                Err(error) => Some(error.to_string()),
            }
        } else {
            reason
        };
        if reason.is_none() {
            released.insert(canonical_or_absolute(&workspace.path));
        }
        report.record(item, reason);
    }

    let owned_paths: BTreeSet<_> = locked
        .state
        .slots
        .iter()
        .map(|slot| slot.path.clone())
        .chain(
            locked
                .state
                .workspaces
                .iter()
                .filter(|workspace| workspace.ownership == WorkspaceOwnership::Acre)
                .map(|workspace| workspace.path.clone()),
        )
        .collect();
    for path in owned_paths {
        // Permission failures and dangling symlinks are not proof that data is gone.
        if !matches!(path.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
            continue;
        }
        let registered = locked.repository.worktree_at(&path);
        let item = CleanItem {
            action: CleanAction::RemoveStaleReference,
            description: format!("stale workspace reference {}", path.display()),
            path: Some(path.clone()),
            branch: registered.and_then(|worktree| worktree.branch.clone()),
            reason: None,
        };
        let reason = if registered.is_some_and(|worktree| worktree.locked || worktree.is_main) {
            Some("worktree is locked or primary".to_owned())
        } else if apply && registered.is_some() {
            remove_worktree(&locked.repository, &path, false)
                .err()
                .map(|error| error.to_string())
        } else {
            None
        };
        if reason.is_none() {
            released.insert(canonical_or_absolute(&path));
            if apply {
                locked.state.slots.retain(|slot| slot.path != path);
                locked.state.workspaces.retain(|workspace| workspace.path != path);
                locked.save(config)?;
            }
        }
        report.record(item, reason);
    }

    locked.repository = discover_repository(&locked.repository.top_level)?;
    // Git can keep a branch in use during a detached rebase or bisect. Never delete refs
    // while an operation is in progress, even when porcelain reports a detached HEAD.
    let mut operation = false;
    for worktree in &locked.repository.worktrees {
        if !worktree.bare && worktree.exists && in_progress_operation(&worktree.path)?.is_some() {
            operation = true;
        }
    }
    for reference in refs
        .iter()
        .filter(|reference| reference.kind == GitRefKind::Local)
    {
        if protected_branches.contains(&reference.short_name) {
            continue;
        }
        let item = CleanItem {
            action: CleanAction::DeleteBranch,
            description: format!("local branch {}", reference.short_name),
            path: None,
            branch: Some(reference.short_name.clone()),
            reason: None,
        };
        let in_use = locked.repository.worktrees.iter().any(|worktree| {
            worktree.branch.as_deref() == Some(&reference.short_name)
                && (apply || !released.contains(&canonical_or_absolute(&worktree.path)))
        }) || locked.state.workspaces.iter().any(|workspace| {
            workspace.target.local_branch.as_deref() == Some(&reference.short_name)
                && !released.contains(&canonical_or_absolute(&workspace.path))
        });
        let reason = if in_use {
            Some("branch still belongs to a retained or external worktree".to_owned())
        } else if operation {
            Some("a Git operation is in progress in this repository".to_owned())
        } else if !is_ancestor(&locked.repository.top_level, &reference.oid, &base.oid)? {
            Some(format!("not merged into {}", report.base))
        } else if apply {
            delete_merged_branch(&locked.repository, reference, base)
                .err()
                .map(|error| error.to_string())
        } else {
            None
        };
        report.record(item, reason);
    }
    Ok(report)
}

fn cleanup_base<'a>(
    repository: &Repository,
    refs: &'a [GitRef],
    requested: Option<&str>,
) -> Result<&'a GitRef> {
    let selected = if let Some(requested) = requested {
        refs.iter()
            .find(|reference| reference.full_name == requested || reference.qualified_name() == requested)
    } else {
        repository.default_branch.as_ref().and_then(|branch| {
            refs.iter()
                .find(|reference| {
                    reference.kind == GitRefKind::Remote
                        && reference.remote == repository.remote
                        && &reference.short_name == branch
                })
                .or_else(|| {
                    refs.iter().find(|reference| {
                        reference.kind == GitRefKind::Local && &reference.short_name == branch
                    })
                })
        })
    };
    selected.ok_or_else(|| {
        AcreError::new(
            "ACRE_CLEAN_BASE_REQUIRED",
            "Choose an existing local or remote branch with --base before cleaning.",
            exit::USAGE,
        )
    })
}
