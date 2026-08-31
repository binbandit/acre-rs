use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::{
    AcreConfig, GitWorktree, Repository, RepositoryState, StoredTarget, TrustLevel,
    WorkspaceLease, WorkspaceOwnership, WorkspaceRecord, WorkspaceSlot, WorkspaceStatus,
};
use crate::state::lock::is_pid_alive;
use crate::state::paths::{active_root, repository_state_path, slots_root};
use crate::util::{is_inside, now_iso, random_short, read_json, write_json};

pub fn load_repository_state(config: &AcreConfig, repository: &Repository) -> Result<RepositoryState> {
    let path = repository_state_path(config, repository);
    let stored = match read_json::<RepositoryState>(&path) {
        Ok(value) => value,
        Err(_) => None,
    };
    let state = match stored {
        Some(state) if state.schema_version == 1 && state.repository_id == repository.id => {
            reconcile_state(state, &repository.worktrees)
        }
        _ => empty_state(repository),
    };
    Ok(recover_owned_worktrees(config, repository, state))
}

pub fn save_repository_state(
    config: &AcreConfig,
    repository: &Repository,
    state: &RepositoryState,
) -> Result<()> {
    let mut state = state.clone();
    state.updated_at = now_iso();
    write_json(&repository_state_path(config, repository), &state)
}

pub fn empty_state(repository: &Repository) -> RepositoryState {
    RepositoryState {
        schema_version: 1,
        repository_id: repository.id.clone(),
        repository_common_dir: repository.common_dir.clone(),
        repository_name: repository.name.clone(),
        updated_at: now_iso(),
        target_paths: BTreeMap::new(),
        slots: Vec::new(),
        workspaces: Vec::new(),
        leases: Vec::new(),
    }
}

pub fn reconcile_state(mut state: RepositoryState, worktrees: &[GitWorktree]) -> RepositoryState {
    let registered: BTreeMap<PathBuf, &GitWorktree> = worktrees
        .iter()
        .map(|worktree| (crate::util::canonical_or_absolute(&worktree.path), worktree))
        .collect();

    for workspace in &mut state.workspaces {
        let path = crate::util::canonical_or_absolute(&workspace.path);
        match registered.get(&path) {
            Some(worktree) if worktree.exists && !worktree.prunable => {}
            _ => workspace.status = WorkspaceStatus::Broken,
        }
    }
    for slot in &mut state.slots {
        let path = crate::util::canonical_or_absolute(&slot.path);
        match registered.get(&path) {
            Some(worktree) if worktree.exists && !worktree.prunable => {}
            _ => slot.status = WorkspaceStatus::Broken,
        }
    }

    let valid: BTreeSet<&str> = state
        .workspaces
        .iter()
        .filter(|workspace| workspace.status != WorkspaceStatus::Broken)
        .map(|workspace| workspace.id.as_str())
        .collect();
    state.leases.retain(|lease| {
        valid.contains(lease.workspace_id.as_str())
            && lease.pid.map(is_pid_alive).unwrap_or(true)
    });
    state.updated_at = now_iso();
    state
}

pub fn find_workspace_by_path<'a>(
    state: &'a RepositoryState,
    target_path: &Path,
) -> Option<&'a WorkspaceRecord> {
    let target_path = crate::util::canonical_or_absolute(target_path);
    state.workspaces.iter().find(|workspace| {
        crate::util::canonical_or_absolute(&workspace.path) == target_path
    })
}

pub fn find_workspace_by_path_mut<'a>(
    state: &'a mut RepositoryState,
    target_path: &Path,
) -> Option<&'a mut WorkspaceRecord> {
    let target_path = crate::util::canonical_or_absolute(target_path);
    state.workspaces.iter_mut().find(|workspace| {
        crate::util::canonical_or_absolute(&workspace.path) == target_path
    })
}

pub fn find_workspace_for_target<'a>(
    state: &'a RepositoryState,
    target: &StoredTarget,
) -> Option<&'a WorkspaceRecord> {
    state.workspaces.iter().find(|workspace| {
        if workspace.status == WorkspaceStatus::Broken {
            return false;
        }
        if let Some(pull_request) = &target.pull_request {
            return workspace.target.pull_request.as_ref().is_some_and(|candidate| {
                candidate.number == pull_request.number
                    && candidate.repository == pull_request.repository
            });
        }
        if let Some(local_branch) = &target.local_branch {
            return workspace.target.local_branch.as_ref() == Some(local_branch);
        }
        workspace.target.kind == target.kind && workspace.target.oid == target.oid
    })
}

pub fn leases_for_workspace<'a>(
    state: &'a RepositoryState,
    workspace_id: &str,
) -> Vec<&'a WorkspaceLease> {
    state
        .leases
        .iter()
        .filter(|lease| lease.workspace_id == workspace_id)
        .collect()
}

fn recover_owned_worktrees(
    config: &AcreConfig,
    repository: &Repository,
    mut state: RepositoryState,
) -> RepositoryState {
    let slot_paths: BTreeSet<PathBuf> = state
        .slots
        .iter()
        .map(|slot| crate::util::canonical_or_absolute(&slot.path))
        .collect();
    let workspace_paths: BTreeSet<PathBuf> = state
        .workspaces
        .iter()
        .map(|workspace| crate::util::canonical_or_absolute(&workspace.path))
        .collect();
    let timestamp = now_iso();

    for worktree in &repository.worktrees {
        let path = crate::util::canonical_or_absolute(&worktree.path);
        if is_inside(&slots_root(config, repository), &path) && !slot_paths.contains(&path) {
            state.slots.push(WorkspaceSlot {
                id: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| random_short(8)),
                path,
                status: if worktree.detached && worktree.exists && !worktree.prunable {
                    WorkspaceStatus::Idle
                } else {
                    WorkspaceStatus::Broken
                },
                environment: None,
                created_at: timestamp.clone(),
                last_used_at: timestamp.clone(),
            });
            continue;
        }
        if is_inside(&active_root(config, repository), &path)
            && !workspace_paths.contains(&path)
        {
            let local_branch = worktree.branch.clone();
            state.workspaces.push(WorkspaceRecord {
                id: format!("recovered-{}", random_short(10)),
                repository_id: repository.id.clone(),
                path,
                ownership: WorkspaceOwnership::Acre,
                status: if worktree.exists && !worktree.prunable {
                    WorkspaceStatus::Retained
                } else {
                    WorkspaceStatus::Broken
                },
                slot_id: None,
                target: StoredTarget {
                    kind: if local_branch.is_some() {
                        crate::model::TargetKind::LocalBranch
                    } else {
                        crate::model::TargetKind::Worktree
                    },
                    display_name: local_branch
                        .clone()
                        .unwrap_or_else(|| format!("detached-{}", &worktree.head[..worktree.head.len().min(8)])),
                    oid: worktree.head.clone(),
                    local_branch,
                    remote_branch: None,
                    pull_request: None,
                },
                trust: TrustLevel::Trusted,
                environment: None,
                baseline_ignored: Vec::new(),
                seeded_paths: Vec::new(),
                baseline_seed_files: Vec::new(),
                activated_at: timestamp.clone(),
                last_used_at: timestamp.clone(),
            });
        }
    }
    state.updated_at = timestamp;
    state
}
