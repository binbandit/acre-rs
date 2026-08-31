use std::collections::BTreeSet;

use crate::error::Result;
use crate::git::operations::{prune_worktrees, remove_worktree, repair_worktrees};
use crate::git::repository::discover_repository;
use crate::git::status::read_status;
use crate::model::{AcreConfig, Repository, RepositoryState, WorkspaceSlot, WorkspaceStatus};
use crate::state::lock::RepositoryLock;
use crate::state::paths::repository_lock_path;
use crate::state::repository::{load_repository_state, save_repository_state};
use crate::util::{age_millis, canonical_or_absolute};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReport {
    pub added_slots: usize,
    pub added_workspaces: usize,
    pub removed_broken_records: usize,
    pub state: RepositoryState,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcSkipped {
    pub slot: WorkspaceSlot,
    pub reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GcReport {
    pub removed: Vec<WorkspaceSlot>,
    pub skipped: Vec<GcSkipped>,
    pub state: RepositoryState,
}

pub fn repair_repository_state(
    config: &AcreConfig,
    repository_input: &Repository,
) -> Result<RepairReport> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let _ = prune_worktrees(repository_input);
    let _ = repair_worktrees(repository_input);
    let repository = discover_repository(&repository_input.top_level)?;
    let before = crate::state::repository::empty_state(&repository);
    let raw_count = before.slots.len() + before.workspaces.len();
    let mut state = load_repository_state(config, &repository)?;
    let registered: BTreeSet<_> = repository
        .worktrees
        .iter()
        .map(|worktree| canonical_or_absolute(&worktree.path))
        .collect();
    let old_slots = state.slots.len();
    let old_workspaces = state.workspaces.len();
    state
        .slots
        .retain(|slot| registered.contains(&canonical_or_absolute(&slot.path)));
    state
        .workspaces
        .retain(|workspace| registered.contains(&canonical_or_absolute(&workspace.path)));
    let removed_broken_records = old_slots + old_workspaces - state.slots.len() - state.workspaces.len();
    let added_slots = state.slots.len().saturating_sub(raw_count);
    let added_workspaces = state.workspaces.len().saturating_sub(raw_count + added_slots);
    save_repository_state(config, &repository, &state)?;
    Ok(RepairReport {
        added_slots,
        added_workspaces,
        removed_broken_records,
        state,
    })
}

pub fn gc_repository(config: &AcreConfig, repository_input: &Repository) -> Result<GcReport> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let mut idle = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .cloned()
        .collect::<Vec<_>>();
    idle.sort_by(|left, right| right.last_used_at.cmp(&left.last_used_at));
    let retained = idle
        .iter()
        .take(config.pool.max_slots)
        .map(|slot| slot.id.clone())
        .collect::<BTreeSet<_>>();
    let retention_ms = u128::from(config.pool.idle_retention_days) * 24 * 60 * 60 * 1000;
    let candidates = idle
        .into_iter()
        .filter(|slot| !retained.contains(&slot.id) || age_millis(&slot.last_used_at) > retention_ms)
        .collect::<Vec<_>>();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for slot in candidates {
        match read_status(&slot.path) {
            Ok(status) if status.dirty => skipped.push(GcSkipped {
                slot,
                reason: "slot is dirty".to_owned(),
            }),
            Ok(_) => match remove_worktree(&repository, &slot.path, false) {
                Ok(()) => removed.push(slot),
                Err(error) => skipped.push(GcSkipped {
                    slot,
                    reason: error.to_string(),
                }),
            },
            Err(error) => skipped.push(GcSkipped {
                slot,
                reason: error.to_string(),
            }),
        }
    }
    let removed_ids = removed.iter().map(|slot| slot.id.as_str()).collect::<BTreeSet<_>>();
    state.slots.retain(|slot| !removed_ids.contains(slot.id.as_str()));
    save_repository_state(config, &repository, &state)?;
    Ok(GcReport { removed, skipped, state })
}
