//! Repair and garbage collection for a repository's Acre state.

use std::collections::BTreeSet;

use crate::error::Result;
use crate::git::operations::{prune_worktrees, remove_worktree, repair_worktrees};
use crate::git::repository::Repository;
use crate::model::{AcreConfig, RepositoryState, WorkspaceSlot, WorkspaceStatus};
use crate::state::repository::{LockedRepository, save_repository_state, stored_repository_state};
use crate::util::{age_millis, canonical_or_absolute};
use crate::workspace::pool::idle_slot_is_safe;

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

pub fn repair_repository_state(config: &AcreConfig, repository_input: &Repository) -> Result<RepairReport> {
    // Git's own locking covers these; Acre's lock is for the state that gets rebuilt below.
    let _ = prune_worktrees(repository_input);
    let _ = repair_worktrees(repository_input);
    let locked = LockedRepository::open(config, repository_input)?;
    let repository = &locked.repository;
    // Loading recovers unregistered Acre worktrees, so anything absent from the persisted
    // state afterwards was recovered by this repair.
    let stored = stored_repository_state(config, repository);
    let known_slots: BTreeSet<&str> = stored
        .iter()
        .flat_map(|state| state.slots.iter().map(|slot| slot.id.as_str()))
        .collect();
    let known_workspaces: BTreeSet<&str> = stored
        .iter()
        .flat_map(|state| state.workspaces.iter().map(|workspace| workspace.id.as_str()))
        .collect();
    let mut state = locked.state.clone();
    // After prune, anything git no longer registers is gone for good; drop its record.
    let registered: BTreeSet<_> = repository
        .worktrees
        .iter()
        .map(|worktree| canonical_or_absolute(&worktree.path))
        .collect();
    let loaded_records = state.slots.len() + state.workspaces.len();
    state
        .slots
        .retain(|slot| registered.contains(&canonical_or_absolute(&slot.path)));
    state
        .workspaces
        .retain(|workspace| registered.contains(&canonical_or_absolute(&workspace.path)));
    let removed_broken_records = loaded_records - state.slots.len() - state.workspaces.len();
    let added_slots = state
        .slots
        .iter()
        .filter(|slot| !known_slots.contains(slot.id.as_str()))
        .count();
    let added_workspaces = state
        .workspaces
        .iter()
        .filter(|workspace| !known_workspaces.contains(workspace.id.as_str()))
        .count();
    save_repository_state(config, repository, &state)?;
    Ok(RepairReport {
        added_slots,
        added_workspaces,
        removed_broken_records,
        state,
    })
}

pub fn gc_repository(config: &AcreConfig, repository_input: &Repository) -> Result<GcReport> {
    let mut locked = LockedRepository::open(config, repository_input)?;
    let mut idle = locked
        .state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .cloned()
        .collect::<Vec<_>>();
    // Newest first, so the first max_slots survive the cut.
    idle.sort_by(|left, right| right.last_used_at.cmp(&left.last_used_at));
    let retained = idle
        .iter()
        .take(config.pool.max_slots)
        .map(|slot| slot.id.clone())
        .collect::<BTreeSet<_>>();
    let retention_ms = u128::from(config.pool.idle_retention_days) * 24 * 60 * 60 * 1000;
    let candidates = idle
        .into_iter()
        // Overflow goes regardless of age; retained slots go only once stale.
        .filter(|slot| !retained.contains(&slot.id) || age_millis(&slot.last_used_at) > retention_ms)
        .collect::<Vec<_>>();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for slot in candidates {
        // Look before removing: a slot someone edited by hand is kept and reported.
        if !idle_slot_is_safe(config, &locked.repository, &slot) {
            skipped.push(GcSkipped {
                slot,
                reason: "slot is in use, locked, changed, or unverified".to_owned(),
            });
            continue;
        }
        match remove_worktree(&locked.repository, &slot.path, false) {
            Ok(()) => removed.push(slot),
            Err(error) => skipped.push(GcSkipped {
                slot,
                reason: error.to_string(),
            }),
        }
    }
    let removed_ids = removed
        .iter()
        .map(|slot| slot.id.as_str())
        .collect::<BTreeSet<_>>();
    locked
        .state
        .slots
        .retain(|slot| !removed_ids.contains(slot.id.as_str()));
    locked.save(config)?;
    Ok(GcReport {
        removed,
        skipped,
        state: locked.state,
    })
}
