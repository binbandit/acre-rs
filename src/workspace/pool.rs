//! The warm pool: stable detached worktrees with reusable dependency and build caches.

use std::path::{Path, PathBuf};

use crate::environment::clone::clone_environment;
use crate::environment::definitions::ALL_FINGERPRINT_FILES;
use crate::environment::fingerprint::{EnvironmentPlan, build_environment_plan};
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::inspect_ignored;
use crate::error::Result;
use crate::git::operations::create_detached_worktree;
use crate::git::refs::resolve_oid;
use crate::git::repository::Repository;
use crate::git::status::{in_progress_operation, read_status};
use crate::model::{
    AcreConfig, EnvironmentSnapshot, EnvironmentState, RepositoryState, TrustLevel, WorkspaceSlot,
    WorkspaceStatus,
};
use crate::state::index::remember_repository;
use crate::state::paths::active_root;
use crate::state::repository::LockedRepository;
use crate::util::{ensure_directory, is_inside, now_iso, random_short};
use crate::workspace::process::find_processes_using_path;

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
    let mut candidates: Vec<&WorkspaceSlot> = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .collect();
    let priority = |slot: &WorkspaceSlot| match &slot.environment {
        Some(environment) if environment.fingerprint == plan.fingerprint => 0,
        None => 1,
        Some(environment) if environment.state == EnvironmentState::Cold => 1,
        _ => 2,
    };
    candidates.sort_by(|left, right| {
        priority(left)
            .cmp(&priority(right))
            .then_with(|| match priority(left) {
                0 => right.last_used_at.cmp(&left.last_used_at),
                1 => std::cmp::Ordering::Equal,
                _ => left.last_used_at.cmp(&right.last_used_at),
            })
    });
    // Safety checks inspect files and running processes. Check only candidates we might use,
    // preserving every check for the selected slot and falling through when one is unsafe.
    let chosen = candidates
        .into_iter()
        .find(|slot| idle_slot_is_safe(config, repository, slot))
        .cloned();
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
    ensure_directory(&active_root(config, repository))?;
    let id = random_short(12);
    let slot_path = active_root(config, repository).join(&id);
    // Detached from the start: a slot must never hold a branch that `git branch -d` would refuse to delete.
    create_detached_worktree(repository, &slot_path, oid)?;
    let mut environment = inspect_environment(&slot_path, plan)?;
    if let Some(source) = find_environment_source(config, repository, state, plan, &slot_path) {
        // Cloning is a bonus; a slot without caches is still a usable slot.
        if let Ok(snapshot) = clone_environment(&source, &slot_path, plan) {
            environment = snapshot;
        }
    }
    let timestamp = now_iso();
    let slot = WorkspaceSlot {
        id,
        path: slot_path,
        head: Some(oid.to_owned()),
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
) -> Option<PathBuf> {
    let matches = |environment: &Option<EnvironmentSnapshot>| {
        environment
            .as_ref()
            .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
    };
    let candidates = state
        .workspaces
        .iter()
        // Never clone from an untrusted (fork PR) workspace, whatever its caches look like.
        .filter(|workspace| workspace.status != WorkspaceStatus::Broken
            && workspace.trust == TrustLevel::Trusted && matches(&workspace.environment))
        .map(|workspace| &workspace.path)
        .chain(
            state
                .slots
                .iter()
                .filter(|slot| slot.status == WorkspaceStatus::Idle && matches(&slot.environment))
                .map(|slot| &slot.path),
        )
        .map(PathBuf::as_path)
        .chain(std::iter::once(repository.primary_path()));
    for source in candidates {
        let Some(worktree) = repository.worktree_at(source).filter(|worktree| worktree.exists) else {
            continue;
        };
        if source == excluded_path {
            continue;
        }
        // A recorded fingerprint cannot describe dependencies changed since activation, whether
        // committed or still local. Ordinary source edits do not prevent cache sharing.
        let manifests_unchanged = read_status(source).is_ok_and(|status| {
            status
                .entries
                .iter()
                .flat_map(|entry| std::iter::once(&entry.path).chain(entry.original_path.iter()))
                .all(|path| !ALL_FINGERPRINT_FILES.contains(&path.rsplit('/').next().unwrap_or_default()))
        });
        if manifests_unchanged
            && build_environment_plan(repository, &worktree.head, config)
                .is_ok_and(|source_plan| source_plan.fingerprint == plan.fingerprint)
            && inspect_environment(source, plan).is_ok_and(|snapshot| !snapshot.present_roots.is_empty())
        {
            return Some(source.to_path_buf());
        }
    }
    None
}

/// Idle metadata alone is not permission to reset or delete a checkout someone has edited.
pub fn idle_slot_is_safe(config: &AcreConfig, repository: &Repository, slot: &WorkspaceSlot) -> bool {
    slot.status == WorkspaceStatus::Idle
        && repository.worktree_at(&slot.path).is_some_and(|worktree| {
            worktree.exists
                && worktree.detached
                && !worktree.locked
                && !worktree.prunable
                && slot.head.as_deref() == Some(worktree.head.as_str())
        })
        && slot.environment.as_ref().is_some_and(|environment| {
            inspect_ignored(&slot.path, &environment.cache_roots)
                .is_ok_and(|layout| layout.unknown.is_empty())
        })
        && read_status(&slot.path).is_ok_and(|status| !status.dirty)
        && in_progress_operation(&slot.path).is_ok_and(|operation| operation.is_none())
        && !std::env::current_dir().is_ok_and(|cwd| is_inside(&slot.path, &cwd))
        && (!config.safety.detect_processes || find_processes_using_path(&slot.path, &[]).is_empty())
}
