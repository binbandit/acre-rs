//! The warm pool: idle detached worktrees kept ready so opening a branch costs a move, not a clone.

use std::path::{Path, PathBuf};

use crate::environment::clone::seed_environment;
use crate::environment::fingerprint::{EnvironmentPlan, build_environment_plan};
use crate::environment::inspect::inspect_environment;
use crate::error::Result;
use crate::git::operations::create_detached_worktree;
use crate::git::refs::resolve_oid;
use crate::git::repository::Repository;
use crate::model::{
    AcreConfig, EnvironmentSnapshot, EnvironmentState, RepositoryState, TrustLevel, WorkspaceSlot,
    WorkspaceStatus,
};
use crate::state::index::remember_repository;
use crate::state::paths::slots_root;
use crate::state::repository::LockedRepository;
use crate::util::{ensure_directory, now_iso, random_short};

/// Creates idle slots until `requested_slots` (or the configured minimum) are warm.
pub fn warm_repository(
    config: &AcreConfig,
    repository: &Repository,
    requested_slots: Option<usize>,
) -> Result<Vec<WorkspaceSlot>> {
    let mut locked = LockedRepository::open(config, repository)?;
    let repository = locked.repository.clone();
    let oid = warm_base_oid(&repository)?;
    let plan = build_environment_plan(&repository, &oid, config)?;
    // Never warm past max_slots, or gc would immediately evict what we just built.
    let requested = requested_slots
        .unwrap_or(config.pool.min_slots)
        .min(config.pool.max_slots);
    let mut created = Vec::new();
    while idle_slot_count(&locked.state) < requested {
        created.push(create_slot(config, &repository, &mut locked.state, &oid, &plan)?);
    }
    locked.save(config)?;
    let _ = remember_repository(config, &repository);
    Ok(created)
}

/// The commit warm slots start from: the remote default branch when known, else the local one.
fn warm_base_oid(repository: &Repository) -> Result<String> {
    let base_ref = match (&repository.remote, &repository.default_branch) {
        (Some(remote), Some(branch)) => format!("{remote}/{branch}"),
        (_, Some(branch)) => branch.clone(),
        _ => "HEAD".to_owned(),
    };
    // A fresh repo may have no default-branch commit yet; fall back to wherever we stand.
    Ok(resolve_oid(&repository.top_level, &base_ref)?
        .or_else(|| {
            repository
                .current_worktree
                .as_ref()
                .map(|worktree| worktree.head.clone())
        })
        .unwrap_or_else(|| "HEAD".to_owned()))
}

pub fn idle_slot_count(state: &RepositoryState) -> usize {
    state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .count()
}

/// Prefers an idle slot with the same generation, then a cold one that costs nothing to
/// recycle, then the stalest; creates a slot only when none is usable.
pub fn select_slot(
    config: &AcreConfig,
    repository: &Repository,
    state: &mut RepositoryState,
    plan: &EnvironmentPlan,
    oid: &str,
) -> Result<WorkspaceSlot> {
    let healthy: Vec<&WorkspaceSlot> = state
        .slots
        .iter()
        .filter(|slot| {
            // Idle in our records is not enough; the directory must still exist for git.
            slot.status == WorkspaceStatus::Idle
                && repository
                    .worktree_at(&slot.path)
                    .is_some_and(|worktree| worktree.exists)
        })
        .collect();
    let chosen = healthy
        .iter()
        .filter(|slot| {
            slot.environment
                .as_ref()
                .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
        })
        // The most recently used match has the freshest caches.
        .max_by_key(|slot| &slot.last_used_at)
        .or_else(|| {
            healthy.iter().find(|slot| {
                slot.environment
                    .as_ref()
                    .is_none_or(|environment| environment.state == EnvironmentState::Cold)
            })
        })
        // Last resort: recycle the stalest slot, caches and all.
        .or_else(|| healthy.iter().min_by_key(|slot| &slot.last_used_at))
        .map(|slot| (*slot).clone());
    match chosen {
        Some(slot) => Ok(slot),
        None => create_slot(config, repository, state, oid, plan),
    }
}

fn create_slot(
    config: &AcreConfig,
    repository: &Repository,
    state: &mut RepositoryState,
    oid: &str,
    plan: &EnvironmentPlan,
) -> Result<WorkspaceSlot> {
    ensure_directory(&slots_root(config, repository))?;
    let id = random_short(12);
    let slot_path = slots_root(config, repository).join(&id);
    // Detached from the start: a slot must never hold a branch that `git branch -d` would refuse to delete.
    create_detached_worktree(repository, &slot_path, oid)?;
    let mut environment = inspect_environment(&slot_path, plan)?;
    if let Some(source) = find_environment_source(config, repository, state, plan, &slot_path)? {
        // Cloning is a bonus; a slot without caches is still a usable slot.
        if let Ok(seeded) = seed_environment(
            &source,
            &slot_path,
            plan,
            // Untrusted on purpose: a pool slot must never carry seed files.
            TrustLevel::Untrusted,
            &repository.top_level,
        ) {
            environment = seeded.snapshot;
        }
    }
    let timestamp = now_iso();
    let slot = WorkspaceSlot {
        id,
        path: slot_path,
        status: WorkspaceStatus::Idle,
        environment: Some(environment),
        created_at: timestamp.clone(),
        last_used_at: timestamp,
    };
    state.slots.push(slot.clone());
    Ok(slot)
}

/// A worktree whose prepared caches match `plan`: a trusted workspace or slot first, then the
/// primary checkout when it is on the same generation and has something to offer.
pub fn find_environment_source(
    config: &AcreConfig,
    repository: &Repository,
    state: &RepositoryState,
    plan: &EnvironmentPlan,
    excluded_path: &Path,
) -> Result<Option<PathBuf>> {
    let matches = |environment: &Option<EnvironmentSnapshot>| {
        environment
            .as_ref()
            .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
    };
    let candidate = state
        .workspaces
        .iter()
        // Never clone from an untrusted (fork PR) workspace, whatever its caches look like.
        .filter(|workspace| workspace.trust == TrustLevel::Trusted && matches(&workspace.environment))
        .map(|workspace| &workspace.path)
        .chain(
            state
                .slots
                .iter()
                .filter(|slot| matches(&slot.environment))
                .map(|slot| &slot.path),
        )
        .find(|path| path.as_path() != excluded_path && path.exists());
    if let Some(candidate) = candidate {
        return Ok(Some(candidate.clone()));
    }

    let primary = repository.primary_path();
    if primary == excluded_path || !primary.exists() {
        return Ok(None);
    }
    // Fingerprint the primary at its own HEAD; its caches match only if it sits on the same generation.
    let primary_oid = repository
        .worktrees
        .first()
        .map(|worktree| worktree.head.as_str())
        .unwrap_or("HEAD");
    let primary_plan = build_environment_plan(repository, primary_oid, config)?;
    if primary_plan.fingerprint != plan.fingerprint {
        return Ok(None);
    }
    let snapshot = inspect_environment(primary, &primary_plan)?;
    // A primary checkout with nothing installed is not a source, just a matching plan.
    Ok((!snapshot.present_roots.is_empty()).then(|| primary.to_path_buf()))
}
