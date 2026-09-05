//! Decides whether a workspace can be returned: nothing unsaved, nothing unknown, nobody using it.

use crate::environment::fingerprint::build_environment_plan;
use crate::environment::roots::inspect_ignored;
use crate::environment::seed::changed_seed_files;
use crate::error::Result;
use crate::git::repository::{Repository, discover_repository};
use crate::git::status::{WorkingTreeStatus, in_progress_operation, read_status};
use crate::model::{AcreConfig, RepositoryState, TargetKind, WorkspaceLease, WorkspaceRecord};
use crate::state::repository::load_repository_state;
use crate::workspace::missing_workspace;
use crate::workspace::process::{ProcessUse, find_processes_using_path};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoneAssessment {
    pub workspace: WorkspaceRecord,
    pub status: WorkingTreeStatus,
    pub operation: Option<String>,
    pub locked: bool,
    pub detached_commits: bool,
    pub lock_reason: Option<String>,
    pub new_ignored: Vec<String>,
    pub changed_seed_files: Vec<String>,
    pub leases: Vec<WorkspaceLease>,
    pub processes: Vec<ProcessUse>,
    pub safe: bool,
    pub reasons: Vec<String>,
}

/// Loads fresh state and assesses one workspace without taking the repository lock.
pub fn assess_workspace_for_return(
    config: &AcreConfig,
    repository: &Repository,
    workspace_id: &str,
    options: &AssessOptions,
) -> Result<DoneAssessment> {
    let repository = discover_repository(&repository.top_level)?;
    let state = load_repository_state(config, &repository)?;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| missing_workspace(workspace_id))?;
    assess_workspace(&repository, &state, workspace, config, options)
}

#[derive(Debug, Clone, Default)]
pub struct AssessOptions {
    pub allowed_session_id: Option<String>,
    pub ignored_pids: Vec<u32>,
}

pub fn assess_workspace(
    repository: &Repository,
    state: &RepositoryState,
    workspace: &WorkspaceRecord,
    config: &AcreConfig,
    options: &AssessOptions,
) -> Result<DoneAssessment> {
    let status = read_status(&workspace.path)?;
    let operation = in_progress_operation(&workspace.path)?;
    let registered = repository.worktree_at(&workspace.path);
    let cache_roots = match &workspace.environment {
        Some(environment) => environment.cache_roots.clone(),
        // A recovered workspace carries no snapshot, so derive its approved roots from its checkout.
        None => {
            let reference = registered.map_or("HEAD", |worktree| worktree.head.as_str());
            build_environment_plan(repository, reference, config)?.cache_roots
        }
    };
    let ignored = inspect_ignored(&workspace.path, &cache_roots)?.unknown;
    let baseline: std::collections::BTreeSet<&str> =
        workspace.baseline_ignored.iter().map(String::as_str).collect();
    let new_ignored = ignored
        .into_iter()
        // Ignored data that was there at activation is ours; only new arrivals are unknown.
        .filter(|entry| !baseline.contains(entry.as_str()))
        .collect::<Vec<_>>();
    // An edited .env means the user put something there that no other copy has.
    let changed_seed_files = changed_seed_files(&workspace.path, &workspace.baseline_seed_files)?;
    let leases = state
        .leases
        .iter()
        .filter(|lease| {
            // The caller's own lease or session doesn't count against it.
            lease.workspace_id == workspace.id
                && options
                    .allowed_session_id
                    .as_ref()
                    .is_none_or(|session| lease.session_id.as_ref() != Some(session))
        })
        .cloned()
        .collect::<Vec<_>>();
    // Opt-out only: a shell or editor sitting in the directory is the commonest reason a return goes wrong.
    let processes = if config.safety.detect_processes {
        find_processes_using_path(&workspace.path, &options.ignored_pids)
    } else {
        Vec::new()
    };
    let locked = registered.is_some_and(|worktree| worktree.locked);
    let slot_head = workspace
        .slot_id
        .as_ref()
        .and_then(|id| state.slots.iter().find(|slot| &slot.id == id))
        .and_then(|slot| slot.head.as_deref());
    let detached_commits = registered.is_some_and(|worktree| {
        worktree.detached
            && (worktree.head != workspace.target.oid
                || workspace.slot_id.is_none()
                || (workspace.target.kind == TargetKind::Worktree
                    && slot_head != Some(worktree.head.as_str())))
    });
    let lock_reason = registered
        .filter(|worktree| worktree.locked)
        .and_then(|worktree| worktree.lock_reason.clone());
    // Every reason is reported, not just the first, so the user fixes them all in one go.
    let mut reasons = Vec::new();
    if detached_commits {
        reasons.push("create a branch for detached commits before returning this workspace".to_owned());
    }
    if locked {
        reasons.push(match &lock_reason {
            Some(reason) => format!("worktree is locked: {reason}"),
            None => "worktree is locked".to_owned(),
        });
    }
    if status.dirty {
        reasons.push("working tree contains tracked or untracked changes".to_owned());
    }
    if let Some(operation) = &operation {
        reasons.push(format!("a {operation} operation is in progress"));
    }
    if config.safety.block_unknown_ignored_files && !new_ignored.is_empty() {
        reasons.push("new ignored files exist outside Acre cache roots".to_owned());
    }
    if !changed_seed_files.is_empty() {
        reasons.push("seeded local files changed after the workspace was opened".to_owned());
    }
    if !leases.is_empty() {
        reasons.push("another client still holds this workspace".to_owned());
    }
    if !processes.is_empty() {
        reasons.push("another process is still using this workspace".to_owned());
    }
    Ok(DoneAssessment {
        workspace: workspace.clone(),
        status,
        operation,
        locked,
        detached_commits,
        lock_reason,
        new_ignored,
        changed_seed_files,
        leases,
        processes,
        safe: reasons.is_empty(),
        reasons,
    })
}
