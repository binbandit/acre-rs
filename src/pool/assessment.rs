use crate::environment::fingerprint::build_environment_plan;
use crate::environment::roots::inspect_ignored;
use crate::environment::seed::changed_seed_files;
use crate::error::Result;
use crate::git::status::{in_progress_operation, read_status};
use crate::git::worktrees::find_worktree_by_path;
use crate::model::{AcreConfig, DoneAssessment, Repository, RepositoryState, WorkspaceRecord};
use crate::pool::process::find_processes_using_path;

#[derive(Debug, Clone, Default)]
pub struct AssessOptions {
    pub allowed_session_id: Option<String>,
    pub allowed_lease_id: Option<String>,
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
    let registered = find_worktree_by_path(&repository.worktrees, &workspace.path);
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
        .filter(|entry| !baseline.contains(entry.as_str()))
        .collect::<Vec<_>>();
    let changed_seed_files = changed_seed_files(&workspace.path, &workspace.baseline_seed_files)?;
    let leases = state
        .leases
        .iter()
        .filter(|lease| {
            lease.workspace_id == workspace.id
                && options.allowed_lease_id.as_deref() != Some(lease.id.as_str())
                && options.allowed_session_id.as_deref() != lease.session_id.as_deref()
        })
        .cloned()
        .collect::<Vec<_>>();
    let processes = if config.safety.detect_processes {
        find_processes_using_path(&workspace.path, &options.ignored_pids)
    } else {
        Vec::new()
    };
    let locked = registered.is_some_and(|worktree| worktree.locked);
    let lock_reason = registered
        .filter(|worktree| worktree.locked)
        .and_then(|worktree| worktree.lock_reason.clone());
    let mut reasons = Vec::new();
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
        lock_reason,
        new_ignored,
        changed_seed_files,
        leases,
        processes,
        safe: reasons.is_empty(),
        reasons,
    })
}
