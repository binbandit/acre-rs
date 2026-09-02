//! Per-repository state: loading reconciled against Git, recovery of orphaned worktrees, locked access.

use crate::model::TargetKind;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::git::repository::{Repository, discover_repository};
use crate::git::worktrees::GitWorktree;
use crate::model::{
    AcreConfig, RepositoryState, StoredTarget, TrustLevel, WorkspaceOwnership, WorkspaceRecord,
    WorkspaceSlot, WorkspaceStatus,
};
use crate::state::lock::{RepositoryLock, is_pid_alive};
use crate::state::paths::{active_root, repository_lock_path, repository_state_path, slots_root};
use crate::state::storage::{read_json, write_json};
use crate::util::{canonical_or_absolute, is_inside, now_iso, random_short, short_hash};

pub fn load_repository_state(config: &AcreConfig, repository: &Repository) -> Result<RepositoryState> {
    let state = match stored_repository_state(config, repository) {
        Some(state) => reconcile_state(state, &repository.worktrees),
        None => empty_state(repository),
    };
    Ok(recover_owned_worktrees(config, repository, state))
}

/// The state persisted for this repository, before reconciliation and recovery.
pub fn stored_repository_state(config: &AcreConfig, repository: &Repository) -> Option<RepositoryState> {
    read_json::<RepositoryState>(&repository_state_path(config, repository))
        // A corrupt state file reads as no state; recovery rebuilds what it can from git.
        .unwrap_or_default()
        .filter(|state| state.schema_version == 1 && state.repository_id == repository.id)
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

fn reconcile_state(mut state: RepositoryState, worktrees: &[GitWorktree]) -> RepositoryState {
    let registered: BTreeMap<PathBuf, &GitWorktree> = worktrees
        .iter()
        .map(|worktree| (canonical_or_absolute(&worktree.path), worktree))
        .collect();

    // Git is the authority: a record whose worktree is gone or prunable is broken, never silently kept.
    for workspace in &mut state.workspaces {
        let path = canonical_or_absolute(&workspace.path);
        match registered.get(&path) {
            Some(worktree) if worktree.exists && !worktree.prunable => {}
            _ => workspace.status = WorkspaceStatus::Broken,
        }
    }
    for slot in &mut state.slots {
        let path = canonical_or_absolute(&slot.path);
        match registered.get(&path) {
            Some(worktree) if worktree.exists && !worktree.prunable => {}
            _ => slot.status = WorkspaceStatus::Broken,
        }
    }

    // Leases on broken workspaces would block a repair forever.
    let valid: BTreeSet<&str> = state
        .workspaces
        .iter()
        .filter(|workspace| workspace.status != WorkspaceStatus::Broken)
        .map(|workspace| workspace.id.as_str())
        .collect();
    state.leases.retain(|lease| {
        // A lease without a pid (a shell session) can't be probed, so it stays until released.
        valid.contains(lease.workspace_id.as_str()) && lease.pid.map(is_pid_alive).unwrap_or(true)
    });
    state.updated_at = now_iso();
    state
}

impl RepositoryState {
    /// The active workspace at `path`, compared after canonicalization.
    pub fn workspace_at(&self, path: &Path) -> Option<&WorkspaceRecord> {
        let path = canonical_or_absolute(path);
        self.workspaces
            .iter()
            .find(|workspace| canonical_or_absolute(&workspace.path) == path)
    }

    /// The usable workspace already bound to `target`, if any.
    pub fn workspace_for_target(&self, target: &StoredTarget) -> Option<&WorkspaceRecord> {
        self.workspaces.iter().find(|workspace| {
            if workspace.status == WorkspaceStatus::Broken {
                return false;
            }
            // By number and repository, not oid: the head moves with every push, but it's the same PR.
            if let Some(pull_request) = &target.pull_request {
                return workspace.target.pull_request.as_ref().is_some_and(|candidate| {
                    candidate.number == pull_request.number && candidate.repository == pull_request.repository
                });
            }
            if let Some(local_branch) = &target.local_branch {
                return workspace.target.local_branch.as_ref() == Some(local_branch);
            }
            workspace.target.kind == target.kind && workspace.target.oid == target.oid
        })
    }
}

/// Exclusive access to a repository: the lock, Git's current view, and the state loaded under it.
pub struct LockedRepository {
    pub repository: Repository,
    pub state: RepositoryState,
    _lock: RepositoryLock,
}

impl LockedRepository {
    pub fn open(config: &AcreConfig, repository: &Repository) -> Result<Self> {
        let lock = RepositoryLock::acquire(&repository_lock_path(config, repository))?;
        // Rediscover under the lock: another process may have added or pruned worktrees.
        let repository = discover_repository(&repository.top_level)?;
        let state = load_repository_state(config, &repository)?;
        Ok(Self {
            repository,
            state,
            _lock: lock,
        })
    }

    pub fn save(&self, config: &AcreConfig) -> Result<()> {
        save_repository_state(config, &self.repository, &self.state)
    }
}

fn recover_owned_worktrees(
    config: &AcreConfig,
    repository: &Repository,
    mut state: RepositoryState,
) -> RepositoryState {
    let slot_paths: BTreeSet<PathBuf> = state
        .slots
        .iter()
        .map(|slot| canonical_or_absolute(&slot.path))
        .collect();
    let workspace_paths: BTreeSet<PathBuf> = state
        .workspaces
        .iter()
        .map(|workspace| canonical_or_absolute(&workspace.path))
        .collect();
    let timestamp = now_iso();

    for worktree in &repository.worktrees {
        let path = canonical_or_absolute(&worktree.path);
        // Only worktrees under Acre's own roots can be recovered; anything else is external.
        if is_inside(&slots_root(config, repository), &path) && !slot_paths.contains(&path) {
            state.slots.push(WorkspaceSlot {
                id: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| random_short(8)),
                path,
                // A slot with a branch checked out is not idle: somebody bound it and we lost the record.
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
        if is_inside(&active_root(config, repository), &path) && !workspace_paths.contains(&path) {
            let local_branch = worktree.branch.clone();
            // The id derives from the path so every load agrees on the identity of a
            // workspace that was never persisted.
            state.workspaces.push(WorkspaceRecord {
                id: format!("recovered-{}", short_hash(path.to_string_lossy().as_bytes(), 10)),
                repository_id: repository.id.clone(),
                path,
                ownership: WorkspaceOwnership::Acre,
                status: if worktree.exists && !worktree.prunable {
                    WorkspaceStatus::Retained
                } else {
                    WorkspaceStatus::Broken
                },
                // No slot to return to, so a recovered workspace is removed on done rather than pooled.
                slot_id: None,
                target: StoredTarget {
                    kind: if local_branch.is_some() {
                        TargetKind::LocalBranch
                    } else {
                        TargetKind::Worktree
                    },
                    // A recovered detached worktree is named by its commit; there is nothing better to call it.
                    display_name: local_branch.clone().unwrap_or_else(|| {
                        format!("detached-{}", &worktree.head[..worktree.head.len().min(8)])
                    }),
                    oid: worktree.head.clone(),
                    local_branch,
                    remote_branch: None,
                    pull_request: None,
                },
                // Trust only affects pooling and seeding; with no slot and no seeded paths, neither can happen.
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
