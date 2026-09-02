use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::environment::clone::{clear_cache_roots, clear_seed_files, seed_environment, seed_files_only};
use crate::environment::fingerprint::build_environment_plan;
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::inspect_ignored;
use crate::environment::seed::snapshot_seed_files;
use crate::error::{AcreError, Result, exit};
use crate::git::operations::{
    bind_target, create_detached_worktree, delete_branch_if_expected, detach_workspace, move_worktree,
    remove_worktree, reset_workspace, restore_stored_target,
};
use crate::git::repository::discover_repository;
use crate::model::{
    AcreConfig, DoneAssessment, EnvironmentPlan, EnvironmentSnapshot, EnvironmentState,
    MaterializedWorkspace, Repository, RepositoryState, ResolvedTarget, StoredTarget, TrustLevel,
    WorkspaceLease, WorkspaceOwnership, WorkspaceRecord, WorkspaceSlot, WorkspaceStatus,
};
use crate::pool::assessment::{AssessOptions, assess_workspace};
use crate::state::config::load_repo_config;
use crate::state::index::remember_repository;
use crate::state::leases::{AcquireLease, acquire_lease, release_lease, release_session_lease};
use crate::state::lock::RepositoryLock;
use crate::state::paths::{active_root, repository_lock_path, slots_root};
use crate::state::repository::{
    find_workspace_by_path, find_workspace_for_target, load_repository_state, save_repository_state,
};
use crate::target::store_target;
use crate::util::{
    branch_slug, canonical_or_absolute, ensure_directory, now_iso, random_short, remove_path, short_hash,
};

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub holder: String,
    pub pid: Option<u32>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MaterializeOptions {
    pub lease: Option<LeaseRequest>,
    pub no_replenish: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReturnOptions {
    pub assessment: AssessOptions,
    pub remove_lease_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReturnResult {
    pub assessment: DoneAssessment,
    pub returned: bool,
    pub pooled: bool,
    pub external: bool,
    pub path: Option<PathBuf>,
}

pub fn materialize_workspace(
    config: &AcreConfig,
    repository_input: &Repository,
    target: &ResolvedTarget,
    options: &MaterializeOptions,
) -> Result<MaterializedWorkspace> {
    let started = Instant::now();
    let lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let repo_config = load_repo_config(&repository.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let mut replenish = false;

    let result = (|| -> Result<MaterializedWorkspace> {
        if let Some(existing_worktree) = &target.existing_worktree {
            let workspace = find_workspace_by_path(&state, &existing_worktree.path)
                .cloned()
                .unwrap_or_else(|| {
                    let workspace = external_workspace(&repository, target, &existing_worktree.path);
                    state.workspaces.push(workspace.clone());
                    workspace
                });
            let lease = attach_lease(&mut state, &workspace, options.lease.as_ref());
            save_repository_state(config, &repository, &state)?;
            let _ = remember_repository(config, &repository);
            return Ok(MaterializedWorkspace {
                repository: repository.clone(),
                target: target.clone(),
                path: workspace.path.clone(),
                workspace,
                created: false,
                reused: true,
                elapsed_ms: started.elapsed().as_millis(),
                lease,
            });
        }

        let stored_target = store_target(target);
        if let Some(active) = find_workspace_for_target(&state, &stored_target).cloned() {
            if active.path.exists() {
                if target.kind == crate::model::TargetKind::NewBranch {
                    return Err(AcreError::new(
                        "ACRE_BRANCH_EXISTS",
                        format!("{} already exists.", target.display_name),
                        exit::CONFLICT,
                    )
                    .with_details(serde_json::json!({
                        "branch": target.display_name,
                        "path": active.path,
                    })));
                }
                let lease = attach_lease(&mut state, &active, options.lease.as_ref());
                save_repository_state(config, &repository, &state)?;
                return Ok(MaterializedWorkspace {
                    repository: repository.clone(),
                    target: target.clone(),
                    path: active.path.clone(),
                    workspace: active,
                    created: false,
                    reused: true,
                    elapsed_ms: started.elapsed().as_millis(),
                    lease,
                });
            }
        }

        let plan = build_environment_plan(&repository, &target.oid, config, &repo_config)?;
        let (next_state, mut slot) = select_slot(config, &repository, state.clone(), &plan, &target.oid)?;
        state = next_state;
        let matching_environment = slot
            .environment
            .as_ref()
            .is_some_and(|environment| environment.fingerprint == plan.fingerprint);

        if !matching_environment {
            let roots = slot
                .environment
                .as_ref()
                .map(|environment| environment.cache_roots.clone())
                .unwrap_or_default()
                .into_iter()
                .chain(plan.cache_roots.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            clear_cache_roots(&slot.path, &roots)?;
            reset_workspace(&slot.path, &target.oid)?;
        }

        let (next_state, active_path) = choose_active_path(config, &repository, target, state.clone())?;
        state = next_state;
        let idle_path = slot.path.clone();
        move_worktree(&repository, &idle_path, &active_path)?;
        let mut seeded_paths = Vec::new();

        let activation = (|| -> Result<(WorkspaceRecord, Option<WorkspaceLease>)> {
            bind_target(&repository, &active_path, target)?;
            let trusted_seed_source = repository
                .worktrees
                .first()
                .map(|worktree| worktree.path.as_path())
                .unwrap_or(repository.top_level.as_path());

            let environment: EnvironmentSnapshot;
            if matching_environment {
                let mut inspected = inspect_environment(&active_path, &plan);
                inspected.source = Some(idle_path.clone());
                inspected.clone_mode = Some(crate::model::CloneMode::Reuse);
                environment = inspected;
                seeded_paths =
                    seed_files_only(trusted_seed_source, &active_path, &plan.seed_files, target.trust)?;
            } else if let Some(source) =
                find_environment_source(config, &repository, &state, &plan, &active_path)?
            {
                match seed_environment(&source, &active_path, &plan, target.trust, trusted_seed_source) {
                    Ok(seeded) => {
                        environment = seeded.snapshot;
                        seeded_paths = seeded.seeded_files;
                    }
                    Err(_) => {
                        environment = inspect_environment(&active_path, &plan);
                    }
                }
            } else {
                seeded_paths =
                    seed_files_only(trusted_seed_source, &active_path, &plan.seed_files, target.trust)?;
                environment = inspect_environment(&active_path, &plan);
            }

            let baseline_ignored = inspect_ignored(&active_path, &environment.cache_roots)?.unknown;
            let baseline_seed_files = snapshot_seed_files(&active_path, &seeded_paths)?;
            let timestamp = now_iso();
            let workspace = WorkspaceRecord {
                id: random_short(24),
                repository_id: repository.id.clone(),
                path: active_path.clone(),
                ownership: WorkspaceOwnership::Acre,
                status: WorkspaceStatus::Active,
                slot_id: Some(slot.id.clone()),
                target: stored_target.clone(),
                trust: target.trust,
                environment: Some(environment.clone()),
                baseline_ignored,
                seeded_paths: seeded_paths.clone(),
                baseline_seed_files,
                activated_at: timestamp.clone(),
                last_used_at: timestamp.clone(),
            };
            slot.path = active_path.clone();
            slot.status = WorkspaceStatus::Active;
            slot.environment = Some(environment);
            slot.last_used_at = timestamp;
            for candidate in &mut state.slots {
                if candidate.id == slot.id {
                    *candidate = slot.clone();
                }
            }
            state.workspaces.retain(|candidate| {
                candidate.id != workspace.id
                    && canonical_or_absolute(&candidate.path) != canonical_or_absolute(&active_path)
            });
            state.workspaces.push(workspace.clone());
            let lease = attach_lease(&mut state, &workspace, options.lease.as_ref());
            save_repository_state(config, &repository, &state)?;
            let _ = remember_repository(config, &repository);
            replenish = config.pool.replenish
                && !options.no_replenish
                && state
                    .slots
                    .iter()
                    .filter(|slot| slot.status == WorkspaceStatus::Idle)
                    .count()
                    < config.pool.min_slots;
            Ok((workspace, lease))
        })();

        match activation {
            Ok((workspace, lease)) => Ok(MaterializedWorkspace {
                repository: repository.clone(),
                target: target.clone(),
                path: active_path,
                workspace,
                created: true,
                reused: matching_environment,
                elapsed_ms: started.elapsed().as_millis(),
                lease,
            }),
            Err(error) => {
                if active_path.exists() {
                    let _ = detach_workspace(&active_path);
                    let _ = clear_seed_files(&active_path, &seeded_paths);
                    if !idle_path.exists() {
                        let _ = move_worktree(&repository, &active_path, &idle_path);
                    }
                }
                if matches!(
                    target.kind,
                    crate::model::TargetKind::NewBranch | crate::model::TargetKind::RemoteBranch
                ) {
                    if let Some(branch) = &target.local_branch {
                        let _ = delete_branch_if_expected(&repository, branch, &target.oid);
                    }
                }
                Err(error)
            }
        }
    })();

    drop(lock);
    if replenish && result.is_ok() {
        launch_replenish(&repository.common_dir);
    }
    result
}

pub fn warm_repository(
    config: &AcreConfig,
    repository_input: &Repository,
    requested_slots: Option<usize>,
) -> Result<Vec<WorkspaceSlot>> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let repo_config = load_repo_config(&repository.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let base_ref = match (&repository.remote, &repository.default_branch) {
        (Some(remote), Some(branch)) => format!("{remote}/{branch}"),
        (_, Some(branch)) => branch.clone(),
        _ => "HEAD".to_owned(),
    };
    let oid = crate::git::refs::resolve_oid(&repository.top_level, &base_ref)?
        .or_else(|| {
            repository
                .current_worktree
                .as_ref()
                .map(|worktree| worktree.head.clone())
        })
        .unwrap_or_else(|| "HEAD".to_owned());
    let plan = build_environment_plan(&repository, &oid, config, &repo_config)?;
    let requested = requested_slots
        .unwrap_or(config.pool.min_slots)
        .min(config.pool.max_slots);
    let mut created = Vec::new();
    while state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .count()
        < requested
    {
        let (next, slot) = create_slot(config, &repository, state, &oid, &plan)?;
        state = next;
        created.push(slot);
    }
    save_repository_state(config, &repository, &state)?;
    remember_repository(config, &repository)?;
    Ok(created)
}

pub fn assess_workspace_for_return(
    config: &AcreConfig,
    repository_input: &Repository,
    workspace_id: &str,
    options: &AssessOptions,
) -> Result<DoneAssessment> {
    let repository = discover_repository(&repository_input.top_level)?;
    let state = load_repository_state(config, &repository)?;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "That Acre workspace no longer exists.",
                exit::NOT_FOUND,
            )
            .with_details(serde_json::json!({ "workspaceId": workspace_id }))
        })?;
    assess_workspace(&repository, &state, workspace, config, options)
}

pub fn return_workspace(
    config: &AcreConfig,
    repository_input: &Repository,
    workspace_id: &str,
    options: &ReturnOptions,
) -> Result<ReturnResult> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let repo_config = load_repo_config(&repository.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "That Acre workspace no longer exists.",
                exit::NOT_FOUND,
            )
            .with_details(serde_json::json!({ "workspaceId": workspace_id }))
        })?;

    if let Some(lease_id) = &options.remove_lease_id {
        let _ = release_lease(&mut state, lease_id)?;
    }
    if let Some(session_id) = &options.assessment.allowed_session_id {
        release_session_lease(&mut state, session_id, Some(&workspace.id));
    }

    if workspace.ownership == WorkspaceOwnership::External {
        state.workspaces.retain(|candidate| candidate.id != workspace.id);
        state.leases.retain(|lease| lease.workspace_id != workspace.id);
        save_repository_state(config, &repository, &state)?;
        let assessment = assess_workspace(&repository, &state, &workspace, config, &options.assessment)?;
        return Ok(ReturnResult {
            assessment,
            returned: false,
            pooled: false,
            external: true,
            path: Some(workspace.path),
        });
    }

    let assessment = assess_workspace(&repository, &state, &workspace, config, &options.assessment)?;
    if !assessment.safe {
        save_repository_state(config, &repository, &state)?;
        return Ok(ReturnResult {
            assessment,
            returned: false,
            pooled: false,
            external: false,
            path: Some(workspace.path),
        });
    }

    let registered = repository
        .worktrees
        .iter()
        .find(|worktree| canonical_or_absolute(&worktree.path) == canonical_or_absolute(&workspace.path))
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKTREE_MISSING",
                "Git no longer knows about this Acre workspace.",
                exit::CONFLICT,
            )
            .with_details(serde_json::json!({ "path": workspace.path }))
        })?;
    let plan = build_environment_plan(&repository, &registered.head, config, &repo_config)?;
    let environment = inspect_environment(&workspace.path, &plan);

    let idle_slots = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .cloned()
        .collect::<Vec<_>>();
    let matching_idle = idle_slots.iter().any(|slot| {
        slot.environment
            .as_ref()
            .is_some_and(|candidate| candidate.fingerprint == environment.fingerprint)
    });
    let mut keep = workspace.trust == TrustLevel::Trusted
        && (idle_slots.len() < config.pool.max_slots
            || (!matching_idle && environment.state != EnvironmentState::Cold));

    if keep && idle_slots.len() >= config.pool.max_slots {
        if let Some(victim) = idle_slots.iter().min_by_key(|slot| slot.last_used_at.clone()) {
            if remove_worktree(&repository, &victim.path, false).is_ok() {
                state.slots.retain(|slot| slot.id != victim.id);
            } else {
                keep = false;
            }
        }
    }

    let slot = workspace
        .slot_id
        .as_ref()
        .and_then(|slot_id| state.slots.iter().find(|slot| &slot.id == slot_id))
        .cloned();
    if slot.is_none() {
        keep = false;
    }
    let mut final_path = None;

    let return_result = (|| -> Result<()> {
        detach_workspace(&workspace.path)?;
        if keep {
            let mut slot = slot.clone().expect("slot checked above");
            let idle_path = slots_root(config, &repository).join(&slot.id);
            if idle_path.exists() {
                remove_path(&idle_path)?;
            }
            move_worktree(&repository, &workspace.path, &idle_path)?;
            clear_seed_files(&idle_path, &workspace.seeded_paths)?;
            slot.path = idle_path.clone();
            slot.status = WorkspaceStatus::Idle;
            slot.environment = Some(EnvironmentSnapshot {
                source: Some(idle_path.clone()),
                clone_mode: Some(crate::model::CloneMode::Reuse),
                ..environment.clone()
            });
            slot.last_used_at = now_iso();
            for candidate in &mut state.slots {
                if candidate.id == slot.id {
                    *candidate = slot.clone();
                }
            }
            final_path = Some(idle_path);
        } else {
            remove_worktree(&repository, &workspace.path, false)?;
            state
                .slots
                .retain(|slot| workspace.slot_id.as_ref() != Some(&slot.id));
        }
        state.workspaces.retain(|candidate| candidate.id != workspace.id);
        state.leases.retain(|lease| lease.workspace_id != workspace.id);
        save_repository_state(config, &repository, &state)
    })();

    if let Err(error) = return_result {
        let seed_source = repository
            .worktrees
            .first()
            .map(|worktree| worktree.path.as_path())
            .unwrap_or(repository.top_level.as_path());
        let _ = restore_workspace_after_failed_return(
            &repository,
            &workspace,
            seed_source,
            final_path.as_deref(),
        );
        return Err(error);
    }

    Ok(ReturnResult {
        assessment,
        returned: true,
        pooled: keep,
        external: false,
        path: final_path,
    })
}

pub fn lease_workspace_by_path(
    config: &AcreConfig,
    repository_input: &Repository,
    workspace_path: &Path,
    request: &LeaseRequest,
) -> Result<Option<WorkspaceLease>> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let Some(workspace) = find_workspace_by_path(&state, workspace_path).cloned() else {
        return Ok(None);
    };
    let lease = attach_lease(&mut state, &workspace, Some(request));
    save_repository_state(config, &repository, &state)?;
    Ok(lease)
}

pub fn release_shell_session_lease(
    config: &AcreConfig,
    repository_input: &Repository,
    session_id: &str,
) -> Result<()> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let before = state.leases.len();
    release_session_lease(&mut state, session_id, None);
    if state.leases.len() != before {
        save_repository_state(config, &repository, &state)?;
    }
    Ok(())
}

pub fn release_workspace_lease(
    config: &AcreConfig,
    repository_input: &Repository,
    lease_id: &str,
) -> Result<(WorkspaceLease, RepositoryState, WorkspaceRecord)> {
    let _lock = RepositoryLock::acquire(&repository_lock_path(config, repository_input))?;
    let repository = discover_repository(&repository_input.top_level)?;
    let mut state = load_repository_state(config, &repository)?;
    let lease = release_lease(&mut state, lease_id)?;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == lease.workspace_id)
        .cloned()
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "The leased workspace no longer exists.",
                exit::NOT_FOUND,
            )
        })?;
    save_repository_state(config, &repository, &state)?;
    Ok((lease, state, workspace))
}

fn attach_lease(
    state: &mut RepositoryState,
    workspace: &WorkspaceRecord,
    request: Option<&LeaseRequest>,
) -> Option<WorkspaceLease> {
    let request = request?;
    if let Some(session_id) = &request.session_id {
        release_session_lease(state, session_id, None);
    }
    Some(acquire_lease(
        state,
        AcquireLease {
            workspace_id: workspace.id.clone(),
            holder: request.holder.clone(),
            pid: request.pid,
            session_id: request.session_id.clone(),
        },
    ))
}

fn select_slot(
    config: &AcreConfig,
    repository: &Repository,
    state: RepositoryState,
    plan: &EnvironmentPlan,
    oid: &str,
) -> Result<(RepositoryState, WorkspaceSlot)> {
    let healthy = state
        .slots
        .iter()
        .filter(|slot| {
            slot.status == WorkspaceStatus::Idle
                && repository.worktrees.iter().any(|worktree| {
                    canonical_or_absolute(&worktree.path) == canonical_or_absolute(&slot.path)
                        && worktree.exists
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    if let Some(slot) = healthy
        .iter()
        .filter(|slot| {
            slot.environment
                .as_ref()
                .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
        })
        .max_by_key(|slot| slot.last_used_at.clone())
    {
        return Ok((state, slot.clone()));
    }
    if let Some(slot) = healthy.iter().find(|slot| {
        slot.environment
            .as_ref()
            .is_none_or(|environment| environment.state == EnvironmentState::Cold)
    }) {
        return Ok((state, slot.clone()));
    }
    if let Some(slot) = healthy.iter().min_by_key(|slot| slot.last_used_at.clone()) {
        return Ok((state, slot.clone()));
    }
    create_slot(config, repository, state, oid, plan)
}

fn create_slot(
    config: &AcreConfig,
    repository: &Repository,
    mut state: RepositoryState,
    oid: &str,
    plan: &EnvironmentPlan,
) -> Result<(RepositoryState, WorkspaceSlot)> {
    ensure_directory(&slots_root(config, repository))?;
    let id = random_short(12);
    let slot_path = slots_root(config, repository).join(&id);
    create_detached_worktree(repository, &slot_path, oid)?;
    let mut environment = inspect_environment(&slot_path, plan);
    if let Some(source) = find_environment_source(config, repository, &state, plan, &slot_path)? {
        if let Ok(seeded) = seed_environment(
            &source,
            &slot_path,
            plan,
            TrustLevel::Untrusted,
            repository.top_level.as_path(),
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
    Ok((state, slot))
}

fn find_environment_source(
    config: &AcreConfig,
    repository: &Repository,
    state: &RepositoryState,
    plan: &EnvironmentPlan,
    excluded_path: &Path,
) -> Result<Option<PathBuf>> {
    let mut candidates = state
        .workspaces
        .iter()
        .filter(|workspace| {
            workspace.trust == TrustLevel::Trusted
                && workspace
                    .environment
                    .as_ref()
                    .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
        })
        .map(|workspace| workspace.path.clone())
        .chain(
            state
                .slots
                .iter()
                .filter(|slot| {
                    slot.environment
                        .as_ref()
                        .is_some_and(|environment| environment.fingerprint == plan.fingerprint)
                })
                .map(|slot| slot.path.clone()),
        )
        .collect::<Vec<_>>();
    candidates.retain(|candidate| candidate != excluded_path && candidate.exists());
    if let Some(candidate) = candidates.into_iter().next() {
        return Ok(Some(candidate));
    }

    let primary = repository
        .worktrees
        .first()
        .map(|worktree| worktree.path.clone())
        .unwrap_or_else(|| repository.top_level.clone());
    if primary == excluded_path || !primary.exists() {
        return Ok(None);
    }
    let repo_config = load_repo_config(&repository.top_level)?;
    let primary_oid = repository
        .worktrees
        .first()
        .map(|worktree| worktree.head.as_str())
        .unwrap_or("HEAD");
    let primary_plan = build_environment_plan(repository, primary_oid, config, &repo_config)?;
    if primary_plan.fingerprint != plan.fingerprint {
        return Ok(None);
    }
    let snapshot = inspect_environment(&primary, &primary_plan);
    Ok((!snapshot.present_roots.is_empty()).then_some(primary))
}

fn choose_active_path(
    config: &AcreConfig,
    repository: &Repository,
    target: &ResolvedTarget,
    mut state: RepositoryState,
) -> Result<(RepositoryState, PathBuf)> {
    let root = active_root(config, repository);
    ensure_directory(&root)?;
    let key = target_path_key(target);
    if let Some(path) = state.target_paths.get(&key) {
        if !path.exists() {
            return Ok((state.clone(), path.clone()));
        }
    }
    let name = target
        .local_branch
        .as_deref()
        .map(ToOwned::to_owned)
        .or_else(|| {
            target
                .pull_request
                .as_ref()
                .map(|pull_request| format!("pr-{}", pull_request.number))
        })
        .unwrap_or_else(|| target.display_name.clone());
    let base = root.join(branch_slug(&name));
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
    Ok((state, chosen))
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

fn restore_workspace_after_failed_return(
    repository: &Repository,
    workspace: &WorkspaceRecord,
    seed_source: &Path,
    moved_path: Option<&Path>,
) -> Result<()> {
    if let Some(moved_path) = moved_path {
        if moved_path.exists() && !workspace.path.exists() {
            move_worktree(repository, moved_path, &workspace.path)?;
        }
    }
    if !workspace.path.exists() {
        create_detached_worktree(repository, &workspace.path, &workspace.target.oid)?;
    }
    restore_stored_target(&workspace.path, &workspace.target)?;
    let _ = seed_files_only(
        seed_source,
        &workspace.path,
        &workspace.seeded_paths,
        workspace.trust,
    );
    Ok(())
}

fn launch_replenish(common_dir: &Path) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(executable)
        .arg("__replenish")
        .arg(common_dir)
        .env("ACRE_BACKGROUND", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
