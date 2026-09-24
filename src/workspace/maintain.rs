//! Repair and garbage collection for a repository's Acre state.

use std::collections::BTreeSet;

use crate::error::Result;
use crate::git::operations::remove_worktree;
use crate::git::repository::Repository;
use crate::model::{AcreConfig, RepositoryState, WorkspaceSlot, WorkspaceStatus};
use crate::state::lock::RepositoryLock;
use crate::state::repository::{
    LockedRepository, quarantine_unreadable_state, save_repository_state, stored_repository_state,
};
use crate::util::{age_millis, canonical_or_absolute};
use crate::workspace::pool::{idle_slot_is_safe, warm_lock_path};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairReport {
    /// Where an unreadable state file was moved before rebuilding from Git.
    pub quarantined_state: Option<std::path::PathBuf>,
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
    let quarantined_state = quarantine_unreadable_state(config, repository_input)?;
    let mut locked = LockedRepository::open(config, repository_input)?;
    let owned = locked.state.slots.iter().map(|slot| &slot.path).chain(
        locked
            .state
            .workspaces
            .iter()
            .filter(|workspace| workspace.ownership == crate::model::WorkspaceOwnership::Acre)
            .map(|workspace| &workspace.path),
    );
    for path in owned {
        if matches!(path.try_exists(), Ok(false)) {
            let _ = remove_worktree(&locked.repository, path, false);
        }
    }
    locked.repository = crate::git::repository::discover_repository(&repository_input.top_level)?;
    let repository = &locked.repository;
    // Loading recovers unregistered Acre worktrees, so anything absent from the persisted
    // state afterwards was recovered by this repair.
    let stored = stored_repository_state(config, repository)?;
    let known_workspaces: BTreeSet<&str> = stored
        .iter()
        .flat_map(|state| state.workspaces.iter().map(|workspace| workspace.id.as_str()))
        .collect();
    let mut state = locked.state.clone();
    // Drop only records whose registration is gone. External registrations are never pruned.
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
    let added_workspaces = state
        .workspaces
        .iter()
        .filter(|workspace| !known_workspaces.contains(workspace.id.as_str()))
        .count();
    save_repository_state(config, repository, &state)?;
    Ok(RepairReport {
        quarantined_state,
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
    // A warm that could not verify its new slot leaves it retained with no workspace. Reclaim those
    // only while no warm is running, since a live one is still copying caches into its slot.
    let warming = RepositoryLock::try_acquire(&warm_lock_path(config, &locked.repository)).ok();
    let abandoned = locked
        .state
        .slots
        .iter()
        .filter(|slot| {
            warming.is_some()
                && slot.status == WorkspaceStatus::Retained
                && locked.state.workspace_at(&slot.path).is_none()
        })
        // Judged by the same checks as an idle slot, as if the warm had finished.
        .map(|slot| WorkspaceSlot {
            status: WorkspaceStatus::Idle,
            ..slot.clone()
        })
        .collect::<Vec<_>>();
    let candidates = idle
        .into_iter()
        // Overflow goes regardless of age; retained slots go only once stale.
        .filter(|slot| !retained.contains(&slot.id) || age_millis(&slot.last_used_at) > retention_ms)
        .chain(abandoned)
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
