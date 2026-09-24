//! The warm pool: stable detached worktrees with reusable dependency and build caches.

use std::path::{Path, PathBuf};

use crate::environment::clone::prepare_environment;
use crate::environment::definitions::is_fingerprint_input;
use crate::environment::fingerprint::{EnvironmentPlan, build_environment_plan};
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::{inspect_ignored, overlaps_seed};
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
use crate::state::lock::RepositoryLock;
use crate::state::paths::{active_root, repository_root};
use crate::state::repository::LockedRepository;
use crate::util::{ensure_directory, is_inside, now_iso, random_short};
use crate::workspace::process::find_processes_using_path;

/// Creates idle slots until `requested_slots` (or the configured minimum) are warm.
pub fn warm_repository(
    config: &AcreConfig,
    repository: &Repository,
    requested_slots: Option<usize>,
) -> Result<Vec<WorkspaceSlot>> {
    // Serialize replenishers separately; cache copying must not lock out foreground commands.
    let _warming = RepositoryLock::try_acquire(&warm_lock_path(config, repository))?;
    // Never warm past max_slots, or gc would immediately evict what we just built.
    let requested = requested_slots
        .unwrap_or(config.pool.min_slots)
        .min(config.pool.max_slots);
    let mut created = Vec::new();
    for _ in 0..requested {
        let mut locked = LockedRepository::open(config, repository)?;
        if idle_slot_count(&locked.state) >= requested {
            break;
        }
        let repository = locked.repository.clone();
        let oid = warm_base_oid(&repository)?;
        let plan = build_environment_plan(&repository, &oid, config)?;
        let mut slot = create_empty_slot(config, &repository, &oid, &plan)?;
        // A crash leaves a retained checkout, never a partially prepared idle slot.
        slot.status = WorkspaceStatus::Retained;
        locked.state.slots.push(slot.clone());
        locked.save(config)?;
        let state = locked.state.clone();
        drop(locked);

        let prepared =
            find_environment_source(config, &repository, &state, &plan, &slot.path).and_then(|source| {
                prepare_environment(&source, &slot.path, &plan)
                    .ok()
                    .map(|copy| (source, copy))
            });

        let mut locked = LockedRepository::open(config, &repository)?;
        let Some(index) =
            locked.state.slots.iter().position(|candidate| {
                candidate.id == slot.id && candidate.status == WorkspaceStatus::Retained
            })
        else {
            continue;
        };
        // An explicit open may adopt the retained checkout while we copy; it is that workspace's now.
        if locked.state.workspace_at(&slot.path).is_some() {
            continue;
        }
        // Manual edits and newly installed caches belong to the user, and a failed check proves
        // nothing. The checkout stays retained for gc to verify, and warming stops rather than
        // leaving another one behind on every run.
        slot.status = WorkspaceStatus::Idle;
        if !idle_slot_is_safe(config, &locked.repository, &slot)
            || !inspect_environment(&slot.path, &plan)?.present_roots.is_empty()
        {
            break;
        }
        slot.environment = Some(
            match prepared
                .filter(|(source, _)| source_matches_plan(config, &locked.repository, source, &plan))
                .and_then(|(_, prepared)| prepared.publish(&slot.path, &plan).ok())
            {
                Some(environment) => environment,
                None => inspect_environment(&slot.path, &plan)?,
            },
        );
        slot.last_used_at = now_iso();
        locked.state.slots[index] = slot.clone();
        locked.save(config)?;
        created.push(slot);
    }
    let _ = remember_repository(config, repository);
    Ok(created)
}

/// Held for the whole of a warm, including the unlocked cache copy into a retained slot.
pub fn warm_lock_path(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("warm.lock")
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
) -> Result<(WorkspaceSlot, bool)> {
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
        .find(|slot| {
            idle_slot_is_safe(config, repository, slot)
                && inspect_ignored(&slot.path, &plan.cache_roots).is_ok_and(|layout| {
                    layout
                        .cache_roots
                        .iter()
                        .all(|root| !overlaps_seed(root, &plan.seed_files))
                })
        })
        .cloned();
    match chosen {
        Some(slot) => Ok((slot, true)),
        None => Ok((create_empty_slot(config, repository, oid, plan)?, false)),
    }
}

fn create_empty_slot(
    config: &AcreConfig,
    repository: &Repository,
    oid: &str,
    plan: &EnvironmentPlan,
) -> Result<WorkspaceSlot> {
    ensure_directory(&active_root(config, repository))?;
    let id = random_short(12);
    let slot_path = active_root(config, repository).join(&id);
    // Detached from the start: a slot must never hold a branch that `git branch -d` would refuse to delete.
    create_detached_worktree(repository, &slot_path, oid)?;
    let environment = inspect_environment(&slot_path, plan)?;
    let timestamp = now_iso();
    Ok(WorkspaceSlot {
        id,
        path: slot_path,
        head: Some(oid.to_owned()),
        status: WorkspaceStatus::Idle,
        environment: Some(environment),
        created_at: timestamp.clone(),
        last_used_at: timestamp,
    })
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
        let Some(_) = repository.worktree_at(source).filter(|worktree| worktree.exists) else {
            continue;
        };
        if source == excluded_path {
            continue;
        }
        if source_matches_plan(config, repository, source, plan)
            && inspect_environment(source, plan).is_ok_and(|snapshot| !snapshot.present_roots.is_empty())
        {
            return Some(source.to_path_buf());
        }
    }
    None
}

/// Read the source again after copying as well as before; its recorded HEAD may be stale.
pub fn source_matches_plan(
    config: &AcreConfig,
    repository: &Repository,
    source: &Path,
    plan: &EnvironmentPlan,
) -> bool {
    let manifests_unchanged = read_status(source).is_ok_and(|status| {
        status
            .entries
            .iter()
            .flat_map(|entry| std::iter::once(&entry.path).chain(entry.original_path.iter()))
            .all(|path| !is_fingerprint_input(path))
    });
    manifests_unchanged
        && resolve_oid(source, "HEAD").ok().flatten().is_some_and(|oid| {
            build_environment_plan(repository, &oid, config)
                .is_ok_and(|source_plan| source_plan.fingerprint == plan.fingerprint)
        })
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
        && (!config.safety.detect_processes
            || find_processes_using_path(&slot.path, &[]).is_ok_and(|processes| processes.is_empty()))
}
