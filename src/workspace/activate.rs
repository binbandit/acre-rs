//! Opens a target in a warm workspace: pick a slot, move it into place, bind the branch, seed
//! caches and local files, and record the result.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::environment::clone::{clear_cache_roots, seed_environment};
use crate::environment::fingerprint::{EnvironmentPlan, build_environment_plan};
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::inspect_ignored;
use crate::environment::seed::{clear_seed_files, seed_files_only, snapshot_seed_files};
use crate::error::{AcreError, Result, exit};
use crate::git::operations::{
    bind_target, delete_branch_if_expected, detach_workspace, move_worktree, reset_workspace,
};
use crate::git::repository::Repository;
use crate::model::{
    AcreConfig, CloneMode, EnvironmentSnapshot, RepositoryState, StoredTarget, TargetKind, WorkspaceLease,
    WorkspaceOwnership, WorkspaceRecord, WorkspaceSlot, WorkspaceStatus,
};
use crate::state::index::remember_repository;
use crate::state::paths::active_root;
use crate::state::repository::{LockedRepository, save_repository_state};
use crate::util::{branch_slug, canonical_or_absolute, ensure_directory, now_iso, random_short, short_hash};
use crate::workspace::lease::{self, LeaseRequest};
use crate::workspace::pool::{find_environment_source, idle_slot_count, select_slot};
use crate::workspace::resolve::ResolvedTarget;

#[derive(Debug, Clone)]
pub struct MaterializedWorkspace {
    pub repository: Repository,
    pub target: ResolvedTarget,
    pub workspace: WorkspaceRecord,
    pub path: PathBuf,
    pub created: bool,
    pub reused: bool,
    pub elapsed_ms: u128,
    pub lease: Option<WorkspaceLease>,
}

#[derive(Debug, Clone, Default)]
pub struct MaterializeOptions {
    pub lease: Option<LeaseRequest>,
    pub no_replenish: bool,
}

pub fn materialize_workspace(
    config: &AcreConfig,
    repository: &Repository,
    target: &ResolvedTarget,
    options: &MaterializeOptions,
) -> Result<MaterializedWorkspace> {
    let started = Instant::now();
    let mut locked = LockedRepository::open(config, repository)?;
    // Rediscovered under the lock; the caller's copy may predate another process's changes.
    let repository = locked.repository.clone();
    let state = &mut locked.state;

    // An existing worktree, Acre-owned or not, is opened where it already is.
    if let Some(existing) = &target.existing_worktree {
        // An existing worktree we've never seen becomes an external record: observed, never mutated.
        let workspace = match state.workspace_at(&existing.path) {
            Some(workspace) => workspace.clone(),
            None => {
                let workspace = external_workspace(&repository, target, &existing.path);
                state.workspaces.push(workspace.clone());
                workspace
            }
        };
        let lease = options
            .lease
            .as_ref()
            .map(|request| lease::acquire(state, &workspace.id, request));
        locked.save(config)?;
        let _ = remember_repository(config, &repository);
        return Ok(materialized(
            &repository,
            target,
            workspace,
            false,
            true,
            started,
            lease,
        ));
    }

    // A workspace already bound to this target is reused rather than duplicated.
    let stored_target = StoredTarget::from(target);
    if let Some(active) = state
        .workspace_for_target(&stored_target)
        // A record whose directory is gone is a broken workspace, not one we can reopen.
        .filter(|active| active.path.exists())
        .cloned()
    {
        // A workspace still claims this branch name even though git lost the ref; don't pile a new branch onto it.
        if target.kind == TargetKind::NewBranch {
            return Err(AcreError::new(
                "ACRE_BRANCH_EXISTS",
                format!("{} already exists.", target.display_name),
                exit::CONFLICT,
            )
            .with_details(serde_json::json!({ "branch": target.display_name, "path": active.path })));
        }
        let lease = options
            .lease
            .as_ref()
            .map(|request| lease::acquire(state, &active.id, request));
        locked.save(config)?;
        return Ok(materialized(
            &repository,
            target,
            active,
            false,
            true,
            started,
            lease,
        ));
    }

    // The plan comes from the target commit's manifests, not from whatever the primary has checked out.
    let plan = build_environment_plan(&repository, &target.oid, config)?;
    let slot = select_slot(config, &repository, state, &plan, &target.oid)?;
    let reused = slot
        .environment
        .as_ref()
        .is_some_and(|environment| environment.fingerprint == plan.fingerprint);
    if !reused {
        // The slot holds another generation's caches: drop them and reset to the target commit.
        let roots: Vec<String> = slot
            .environment
            .iter()
            .flat_map(|environment| environment.cache_roots.iter())
            .chain(plan.cache_roots.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        clear_cache_roots(&slot.path, &roots)?;
        reset_workspace(&slot.path, &target.oid)?;
    }
    let active_path = choose_active_path(config, &repository, target, state)?;
    // Remember where it came from: on failure the slot goes back there.
    let idle_path = slot.path.clone();
    // Move before binding: the branch should never be visible at a pool path.
    move_worktree(&repository, &idle_path, &active_path)?;

    // Filled by activate as it copies, so the rollback below knows exactly what to remove.
    let mut seeded_paths = Vec::new();
    let activation = activate(
        config,
        &repository,
        state,
        target,
        options,
        &plan,
        slot,
        &active_path,
        reused,
        &mut seeded_paths,
    );
    let (workspace, lease) = match activation {
        Ok(activated) => activated,
        Err(error) => {
            rollback(&repository, target, &active_path, &idle_path, &seeded_paths);
            return Err(error);
        }
    };
    let replenish =
        config.pool.replenish && !options.no_replenish && idle_slot_count(state) < config.pool.min_slots;
    let _ = remember_repository(config, &repository);
    // Release the lock before spawning: the replenisher needs it.
    drop(locked);
    if replenish {
        launch_replenish(&repository.common_dir);
    }
    Ok(materialized(
        &repository,
        target,
        workspace,
        true,
        reused,
        started,
        lease,
    ))
}

/// Binds the target inside its active path, seeds caches and local files, and records the
/// workspace. Every local file it copies lands in `seeded_paths` so a failure can undo it.
#[allow(clippy::too_many_arguments)]
fn activate(
    config: &AcreConfig,
    repository: &Repository,
    state: &mut RepositoryState,
    target: &ResolvedTarget,
    options: &MaterializeOptions,
    plan: &EnvironmentPlan,
    mut slot: WorkspaceSlot,
    active_path: &Path,
    reused: bool,
    seeded_paths: &mut Vec<String>,
) -> Result<(WorkspaceRecord, Option<WorkspaceLease>)> {
    // Bind before seeding: the baseline below reads ignore rules, and those belong to the target commit.
    bind_target(repository, active_path, target)?;
    // Seed files always come from the primary checkout, the one copy the user actually maintains.
    let seed_source = repository.primary_path();
    // Three ways to arrive at caches: already in the slot, cloned from a sibling, or none yet.
    let environment = if reused {
        *seeded_paths = seed_files_only(seed_source, active_path, &plan.seed_files, target.trust)?;
        EnvironmentSnapshot {
            source: Some(slot.path.clone()),
            clone_mode: Some(CloneMode::Reuse),
            ..inspect_environment(active_path, plan)?
        }
    } else if let Some(source) = find_environment_source(config, repository, state, plan, active_path)? {
        match seed_environment(&source, active_path, plan, target.trust, seed_source) {
            Ok(seeded) => {
                *seeded_paths = seeded.seeded_files;
                seeded.snapshot
            }
            // A failed clone is not fatal: the workspace opens cold and the user installs as usual.
            Err(_) => inspect_environment(active_path, plan)?,
        }
    } else {
        *seeded_paths = seed_files_only(seed_source, active_path, &plan.seed_files, target.trust)?;
        inspect_environment(active_path, plan)?
    };

    // Whatever is ignored now was ours; only additions count against `done` later.
    let baseline_ignored = inspect_ignored(active_path, &environment.cache_roots)?.unknown;
    // Hash what we copied so `done` can tell an edited .env from an untouched one.
    let baseline_seed_files = snapshot_seed_files(active_path, seeded_paths)?;
    let timestamp = now_iso();
    let workspace = WorkspaceRecord {
        id: random_short(24),
        repository_id: repository.id.clone(),
        path: active_path.to_path_buf(),
        ownership: WorkspaceOwnership::Acre,
        status: WorkspaceStatus::Active,
        slot_id: Some(slot.id.clone()),
        target: StoredTarget::from(target),
        trust: target.trust,
        environment: Some(environment.clone()),
        baseline_ignored,
        seeded_paths: seeded_paths.clone(),
        baseline_seed_files,
        activated_at: timestamp.clone(),
        last_used_at: timestamp.clone(),
    };
    // The slot record follows the directory: it is the same worktree, now active.
    slot.path = active_path.to_path_buf();
    slot.status = WorkspaceStatus::Active;
    slot.environment = Some(environment);
    slot.last_used_at = timestamp;
    if let Some(existing) = state.slots.iter_mut().find(|candidate| candidate.id == slot.id) {
        *existing = slot;
    }
    let active_key = canonical_or_absolute(active_path);
    state
        .workspaces
        // A stale record for this path (a broken earlier activation) would shadow the new one.
        .retain(|candidate| canonical_or_absolute(&candidate.path) != active_key);
    state.workspaces.push(workspace.clone());
    let lease = options
        .lease
        .as_ref()
        .map(|request| lease::acquire(state, &workspace.id, request));
    // Saved inside the fallible section on purpose: if this fails, the rollback still runs.
    save_repository_state(config, repository, state)?;
    Ok((workspace, lease))
}

/// Puts a half-activated slot back where it came from and removes any branch created for it.
fn rollback(
    repository: &Repository,
    target: &ResolvedTarget,
    active_path: &Path,
    idle_path: &Path,
    seeded_paths: &[String],
) {
    if active_path.exists() {
        let _ = detach_workspace(active_path);
        let _ = clear_seed_files(active_path, seeded_paths);
        // The slot path is normally empty after the move; if something reclaimed it, leave both alone.
        if !idle_path.exists() {
            let _ = move_worktree(repository, active_path, idle_path);
        }
    }
    // Only branches we created ourselves, and delete_branch_if_expected still refuses if they moved.
    if matches!(target.kind, TargetKind::NewBranch | TargetKind::RemoteBranch) {
        if let Some(branch) = &target.local_branch {
            let _ = delete_branch_if_expected(repository, branch, &target.oid);
        }
    }
}

fn materialized(
    repository: &Repository,
    target: &ResolvedTarget,
    workspace: WorkspaceRecord,
    created: bool,
    reused: bool,
    started: Instant,
    lease: Option<WorkspaceLease>,
) -> MaterializedWorkspace {
    MaterializedWorkspace {
        repository: repository.clone(),
        target: target.clone(),
        path: workspace.path.clone(),
        workspace,
        created,
        reused,
        elapsed_ms: started.elapsed().as_millis(),
        lease,
    }
}

/// A stable directory for the target under the repository's workspaces root, remembered so the
/// same branch always lands in the same place.
fn choose_active_path(
    config: &AcreConfig,
    repository: &Repository,
    target: &ResolvedTarget,
    state: &mut RepositoryState,
) -> Result<PathBuf> {
    let root = active_root(config, repository);
    ensure_directory(&root)?;
    let key = target_path_key(target);
    if let Some(path) = state.target_paths.get(&key) {
        // The remembered path is free again, so the branch lands where it lived last time.
        if !path.exists() {
            return Ok(path.clone());
        }
    }
    // Branch name first, then PR number, then whatever the target is called.
    let name = target
        .local_branch
        .clone()
        .or_else(|| {
            target
                .pull_request
                .as_ref()
                .map(|pull_request| format!("pr-{}", pull_request.number))
        })
        .unwrap_or_else(|| target.display_name.clone());
    let base = root.join(branch_slug(&name));
    // Another target may own the same slug (feature/x vs feature-x); never share a directory.
    let reserved: BTreeSet<PathBuf> = state
        .target_paths
        .iter()
        .filter(|(stored_key, _)| *stored_key != &key)
        .map(|(_, path)| canonical_or_absolute(path))
        .collect();
    let chosen = if !base.exists() && !reserved.contains(&canonical_or_absolute(&base)) {
        base
    } else {
        PathBuf::from(format!("{}--{}", base.display(), short_hash(&key, 6)))
    };
    state.target_paths.insert(key, chosen.clone());
    Ok(chosen)
}

fn target_path_key(target: &ResolvedTarget) -> String {
    if let Some(branch) = &target.local_branch {
        return format!("branch:{branch}");
    }
    if let Some(pull_request) = &target.pull_request {
        return format!("pr:{}:{}", pull_request.repository, pull_request.number);
    }
    format!("oid:{}", target.oid)
}

fn external_workspace(repository: &Repository, target: &ResolvedTarget, path: &Path) -> WorkspaceRecord {
    let timestamp = now_iso();
    WorkspaceRecord {
        id: format!("external-{}", random_short(16)),
        repository_id: repository.id.clone(),
        path: path.to_path_buf(),
        ownership: WorkspaceOwnership::External,
        status: WorkspaceStatus::Active,
        slot_id: None,
        target: StoredTarget::from(target),
        trust: target.trust,
        environment: None,
        baseline_ignored: Vec::new(),
        seeded_paths: Vec::new(),
        baseline_seed_files: Vec::new(),
        activated_at: timestamp.clone(),
        last_used_at: timestamp,
    }
}

/// Tops the pool back up in a detached background process so the caller returns immediately.
fn launch_replenish(common_dir: &Path) {
    // Best effort: without a path to ourselves the pool simply stays as it is.
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(executable)
        .arg("__replenish")
        .arg(common_dir)
        // Tells the child to swallow failures: nobody is watching its output.
        .env("ACRE_BACKGROUND", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
