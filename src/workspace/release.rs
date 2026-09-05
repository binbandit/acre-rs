//! Returns a workspace: proves it safe, then pools its slot or removes the worktree.

use crate::environment::fingerprint::build_environment_plan;
use crate::environment::inspect::inspect_environment;
use crate::environment::seed::{clear_seed_files, seed_files_only};
use crate::error::{AcreError, Result, exit};
use crate::git::operations::{detach_workspace, remove_worktree, restore_stored_target};
use crate::git::repository::Repository;
use crate::model::{
    AcreConfig, CloneMode, EnvironmentSnapshot, EnvironmentState, RepositoryState, TrustLevel,
    WorkspaceOwnership, WorkspaceRecord, WorkspaceSlot, WorkspaceStatus,
};
use crate::state::repository::{LockedRepository, save_repository_state};
use crate::util::now_iso;
use crate::workspace::assess::{AssessOptions, DoneAssessment, assess_workspace};
use crate::workspace::pool::idle_slot_is_safe;
use crate::workspace::{lease, missing_workspace};

#[derive(Debug, Clone)]
pub struct ReturnResult {
    pub assessment: DoneAssessment,
    pub returned: bool,
    pub pooled: bool,
    pub external: bool,
}

pub fn return_workspace(
    config: &AcreConfig,
    repository: &Repository,
    workspace_id: &str,
    options: &AssessOptions,
) -> Result<ReturnResult> {
    let mut locked = LockedRepository::open(config, repository)?;
    let repository = locked.repository.clone();
    let state = &mut locked.state;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .ok_or_else(|| missing_workspace(workspace_id))?;

    // The caller's own shell lease must not count as "someone else is using it".
    if let Some(session_id) = &options.allowed_session_id {
        lease::release_session(state, session_id, Some(&workspace.id));
    }

    // External worktrees are only ever forgotten, never touched.
    if workspace.ownership == WorkspaceOwnership::External {
        forget_workspace(state, &workspace.id);
        locked.save(config)?;
        let assessment = assess_workspace(&repository, &locked.state, &workspace, config, options)?;
        return Ok(ReturnResult {
            assessment,
            returned: false,
            pooled: false,
            external: true,
        });
    }

    let assessment = assess_workspace(&repository, state, &workspace, config, options)?;
    // Refusing still saves: the lease changes above must stick.
    if !assessment.safe {
        locked.save(config)?;
        return Ok(ReturnResult {
            assessment,
            returned: false,
            pooled: false,
            external: false,
        });
    }

    let registered = repository.worktree_at(&workspace.path).ok_or_else(|| {
        AcreError::new(
            "ACRE_WORKTREE_MISSING",
            "Git no longer knows about this Acre workspace.",
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "path": workspace.path }))
    })?;
    // Fingerprint the commit actually checked out, not the one the record remembers.
    let plan = build_environment_plan(&repository, &registered.head, config)?;
    let environment = inspect_environment(&workspace.path, &plan)?;
    let slot = slot_to_keep(config, &repository, state, &workspace, &environment);
    let pooled = slot.is_some();

    if let Err(error) = pool_or_remove(config, &repository, state, &workspace, environment, slot) {
        let _ = restore_workspace(&repository, &workspace);
        return Err(error);
    }
    Ok(ReturnResult {
        assessment,
        returned: true,
        pooled,
        external: false,
    })
}

/// Whether the workspace's environment is worth keeping warm. Returns its slot record when it
/// is, evicting the stalest idle slot if the pool is full; `None` means remove the worktree.
fn slot_to_keep(
    config: &AcreConfig,
    repository: &Repository,
    state: &mut RepositoryState,
    workspace: &WorkspaceRecord,
    environment: &EnvironmentSnapshot,
) -> Option<WorkspaceSlot> {
    let slot = workspace
        .slot_id
        .as_ref()
        .and_then(|id| state.slots.iter().find(|slot| &slot.id == id))
        .cloned()?;
    // Untrusted caches came from a fork; they never enter the pool.
    if workspace.trust != TrustLevel::Trusted {
        return None;
    }
    let idle: Vec<WorkspaceSlot> = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .cloned()
        .collect();
    let matching_idle = idle.iter().any(|candidate| {
        candidate
            .environment
            .as_ref()
            .is_some_and(|candidate| candidate.fingerprint == environment.fingerprint)
    });
    // Room in the pool, or a warm generation the pool doesn't have yet.
    let worth_keeping =
        idle.len() < config.pool.max_slots || (!matching_idle && environment.state != EnvironmentState::Cold);
    if !worth_keeping {
        return None;
    }
    if idle.len() >= config.pool.max_slots {
        // Full pool: evict the stalest idle slot to make room, but only if git lets it go.
        let victim = idle
            .iter()
            .filter(|slot| idle_slot_is_safe(config, repository, slot))
            .min_by_key(|slot| &slot.last_used_at)?;
        if remove_worktree(repository, &victim.path, false).is_err() {
            return None;
        }
        state.slots.retain(|slot| slot.id != victim.id);
    }
    Some(slot)
}

/// Detaches the branch, then keeps the prepared worktree in place or removes it.
fn pool_or_remove(
    config: &AcreConfig,
    repository: &Repository,
    state: &mut RepositoryState,
    workspace: &WorkspaceRecord,
    environment: EnvironmentSnapshot,
    slot: Option<WorkspaceSlot>,
) -> Result<()> {
    // Detach first so the branch is free to be checked out elsewhere immediately.
    detach_workspace(&workspace.path)?;
    match slot {
        Some(mut slot) => {
            // Secrets never sit in the pool: the next occupant could be an untrusted PR.
            clear_seed_files(&workspace.path, &workspace.seeded_paths)?;
            slot.head = repository
                .worktree_at(&workspace.path)
                .map(|worktree| worktree.head.clone());
            slot.status = WorkspaceStatus::Idle;
            // Recorded as Reuse from its own path: the next opener finds these caches in place.
            slot.environment = Some(EnvironmentSnapshot {
                source: Some(workspace.path.clone()),
                clone_mode: Some(CloneMode::Reuse),
                ..environment
            });
            slot.last_used_at = now_iso();
            if let Some(existing) = state.slots.iter_mut().find(|candidate| candidate.id == slot.id) {
                *existing = slot;
            }
        }
        // No slot to return to, or not worth keeping: the branch survives, the directory does not.
        None => {
            remove_worktree(repository, &workspace.path, false)?;
            state
                .slots
                .retain(|slot| workspace.slot_id.as_ref() != Some(&slot.id));
        }
    }
    forget_workspace(state, &workspace.id);
    save_repository_state(config, repository, state)
}

fn forget_workspace(state: &mut RepositoryState, workspace_id: &str) {
    state.workspaces.retain(|workspace| workspace.id != workspace_id);
    state.leases.retain(|lease| lease.workspace_id != workspace_id);
}

/// Restores the branch and seed files after a failed return.
fn restore_workspace(repository: &Repository, workspace: &WorkspaceRecord) -> Result<()> {
    if !workspace.path.exists() {
        return Ok(());
    }
    // Re-bind the branch we detached, so the user finds the workspace as they left it.
    restore_stored_target(&workspace.path, &workspace.target)?;
    let _ = seed_files_only(
        repository.primary_path(),
        &workspace.path,
        &workspace.seeded_paths,
        workspace.trust,
        &mut Vec::new(),
    );
    Ok(())
}
