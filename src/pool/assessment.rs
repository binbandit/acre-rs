use crate::environment::seed::changed_seed_files;
use crate::error::Result;
use crate::git::status::{in_progress_operation, list_ignored, read_status};
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
    let excluded_roots = workspace
        .environment
        .as_ref()
        .map(|environment| environment.cache_roots.as_slice())
        .unwrap_or(&[]);
    let ignored = list_ignored(&workspace.path, excluded_roots)?;
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
    let registered = repository.worktrees.iter().find(|worktree| {
        crate::util::canonical_or_absolute(&worktree.path)
            == crate::util::canonical_or_absolute(&workspace.path)
    });
    let mut reasons = Vec::new();
    if let Some(worktree) = registered.filter(|worktree| worktree.locked) {
        reasons.push(
            worktree
                .lock_reason
                .as_ref()
                .map(|reason| format!("worktree is locked: {reason}"))
                .unwrap_or_else(|| "worktree is locked".to_owned()),
        );
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
        new_ignored,
        changed_seed_files,
        leases,
        processes,
        safe: reasons.is_empty(),
        reasons,
    })
}
