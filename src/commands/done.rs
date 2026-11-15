//! `acre done`: return a workspace, resuming after the shell has moved out of it when needed.

use std::path::{Path, PathBuf};

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::{Repository, discover_repository, discover_repository_from_common_dir};
use crate::git::status::read_status;
use crate::model::{
    AcreConfig, PendingDoneOperation, RepositoryState, StoredTarget, WorkspaceOwnership, WorkspaceRecord,
};
use crate::shell::directive::write_resume_directive;
use crate::shell::navigation::{navigate_direct, record_shell_move, repository_at};
use crate::state::config::load_config;
use crate::state::operations::{read_pending_done, remove_pending_operation, save_pending_done};
use crate::state::repository::load_repository_state;
use crate::state::shell::read_shell_state;
use crate::ui::output::Renderer;
use crate::util::{canonical_or_absolute, is_inside, now_iso, random_id};
use crate::workspace::assess::{AssessOptions, DoneAssessment, assess_workspace, checked_out_plan};
use crate::workspace::lease::release_shell_session_lease;
use crate::workspace::release::{ReturnResult, return_workspace};
use crate::workspace::resolve::resolve_existing_target;

pub fn run(context: &CommandContext, selector: Option<&str>) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    let target_path = target_path(&repository, &state, selector)?;
    if repository
        .worktree_at(&target_path)
        .is_some_and(|worktree| worktree.is_main)
    {
        return Err(AcreError::new(
            "ACRE_PRIMARY_WORKTREE",
            "The repository primary worktree is permanent; Acre only returns workspaces that it created.",
            exit::REFUSED,
        )
        .with_details(serde_json::json!({ "path": target_path })));
    }
    // A shell standing in the workspace would have it pulled out from under it, so it has to move
    // first. -C changes target lookup, not the directory occupied by the calling shell.
    let shell_directory = std::env::current_dir()?;
    let shell_inside = is_inside(&target_path, &shell_directory).then_some(shell_directory.as_path());

    let owned = state
        .workspace_at(&target_path)
        .filter(|workspace| workspace.ownership == WorkspaceOwnership::Acre);
    let Some(workspace) = owned.cloned() else {
        return leave_untouched(
            context,
            &config,
            &repository,
            &state,
            selector,
            &target_path,
            shell_inside,
        );
    };
    let assess = assess_options(context, context.shell.session_id.clone());
    let Some(shell_directory) = shell_inside else {
        let result = return_workspace(&config, &repository, &workspace.id, &assess)?;
        return Ok(render_return(context, &workspace, &result));
    };

    if context.global.json {
        return Err(AcreError::new(
            "ACRE_SHELL_MOVE_REQUIRED",
            "Run done from outside this workspace in JSON mode.",
            exit::REFUSED,
        ));
    }
    // Prove the return safe before moving the shell; the resumed command proves it again.
    let plan = checked_out_plan(&repository, &workspace, &config)?;
    let assessment = assess_workspace(&repository, &state, &workspace, &plan, &config, &assess)?;
    if !assessment.safe {
        render_unsafe(context, &assessment);
        return Ok(exit::REFUSED);
    }
    if !context.shell.active {
        return Err(AcreError::new(
            "ACRE_SHELL_INTEGRATION_REQUIRED",
            "Acre cannot safely return the workspace containing this shell until shell integration is installed.",
            exit::ENVIRONMENT,
        )
        .with_details(serde_json::json!({ "hint": "acre setup", "workspace": workspace.path })));
    }
    let destination = safe_destination(context, &config, &repository, &workspace.path, shell_directory)?;
    let registered = repository.worktree_at(&workspace.path).ok_or_else(|| {
        AcreError::new(
            "ACRE_WORKTREE_MISSING",
            "Git no longer knows about this Acre workspace.",
            exit::CONFLICT,
        )
    })?;
    // Two phases: the shell moves out first, then calls back with this token to finish.
    let token = random_id();
    save_pending_done(
        &config,
        &PendingDoneOperation {
            schema_version: 1,
            kind: "done".to_owned(),
            token: token.clone(),
            repository_common_dir: repository.common_dir.clone(),
            workspace_id: workspace.id,
            expected_head: registered.head.clone(),
            expected_status_fingerprint: assessment.status.fingerprint,
            safe_destination: destination.clone(),
            original_directory: Some(shell_directory.to_path_buf()),
            current_session_id: context.shell.session_id.clone(),
            created_at: now_iso(),
        },
    )?;
    write_resume_directive(context, &destination, &token)?;
    Ok(exit::RESUME)
}

/// The worktree `done` acts on: the one a selector names, from wherever we stand, or the current one.
fn target_path(repository: &Repository, state: &RepositoryState, selector: Option<&str>) -> Result<PathBuf> {
    let Some(selector) = selector else {
        return repository
            .current_worktree
            .as_ref()
            .map(|worktree| worktree.path.clone())
            .ok_or_else(|| {
                AcreError::new(
                    "ACRE_WORKSPACE_NOT_FOUND",
                    "Acre could not identify the current worktree.",
                    exit::NOT_FOUND,
                )
            });
    };
    let target = resolve_existing_target(repository, state, selector)?;
    target
        .existing_worktree
        .as_ref()
        .map(|worktree| worktree.path.clone())
        // Match by target identity: PR number and repository, or branch. Never by an absent branch.
        .or_else(|| {
            state
                .workspace_for_target(&StoredTarget::from(&target))
                .map(|workspace| workspace.path.clone())
        })
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_ACTIVE",
                format!("{selector} does not currently have a workspace."),
                exit::NOT_FOUND,
            )
        })
}

/// `done` on a worktree Acre has nothing to return: an external one, which is never touched, or
/// an idle slot, which is already in the pool. A shell standing in it is still moved out.
fn leave_untouched(
    context: &CommandContext,
    config: &AcreConfig,
    repository: &Repository,
    state: &RepositoryState,
    selector: Option<&str>,
    target_path: &Path,
    shell_inside: Option<&Path>,
) -> Result<i32> {
    // An idle slot has no workspace record, but it is ours.
    let idle_slot = state.workspace_at(target_path).is_none()
        && state
            .slots
            .iter()
            .any(|slot| canonical_or_absolute(&slot.path) == canonical_or_absolute(target_path));
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": true,
            "action": "done",
            "external": !idle_slot,
            "pooled": idle_slot,
            "retained": !idle_slot,
            "path": target_path,
        }));
        return Ok(exit::SUCCESS);
    }
    let directory_name = |path: &Path| path.file_name().and_then(|name| name.to_str()).map(str::to_owned);
    let name = match shell_inside {
        Some(_) => directory_name(target_path),
        None => selector
            .map(str::to_owned)
            .or_else(|| target_path.to_str().map(str::to_owned)),
    };
    let summary = format!(
        "<dim>{} {} {}.</dim>",
        if idle_slot {
            "Warm slot"
        } else {
            "External worktree"
        },
        renderer.value(name.as_deref().unwrap_or("worktree")),
        if idle_slot {
            "is already in the pool"
        } else {
            "was left untouched"
        },
    );
    let Some(shell_directory) = shell_inside else {
        renderer.line(summary);
        return Ok(exit::SUCCESS);
    };
    let destination = safe_destination(context, config, repository, target_path, shell_directory)?;
    // Drop our own lease before leaving, or the worktree looks in use forever.
    if let Some(session_id) = &context.shell.session_id {
        let _ = release_shell_session_lease(config, repository, session_id);
    }
    renderer.line(summary);
    let label = directory_name(&destination).unwrap_or_else(|| "workspace".to_owned());
    navigate_direct(
        context,
        config,
        &destination,
        repository_at(&destination, repository).as_ref(),
        &label,
    )?;
    Ok(exit::SUCCESS)
}

/// Our own pid and the wrapper shell's are expected in the directory; anything else counts.
fn assess_options(context: &CommandContext, allowed_session_id: Option<String>) -> AssessOptions {
    AssessOptions {
        allowed_session_id,
        ignored_pids: [Some(std::process::id()), context.shell.pid]
            .into_iter()
            .flatten()
            .collect(),
    }
}

pub fn resume(context: &CommandContext, token: &str) -> Result<i32> {
    let config = load_config()?;
    let operation = read_pending_done(&config, token)?.ok_or_else(|| {
        AcreError::new(
            "ACRE_OPERATION_NOT_FOUND",
            "This Acre operation has expired or already completed.",
            exit::NOT_FOUND,
        )
    })?;
    if !context.shell.active || context.shell.session_id != operation.current_session_id {
        return Err(AcreError::new(
            "ACRE_OPERATION_SESSION_MISMATCH",
            "Only the shell that started this operation can resume it.",
            exit::REFUSED,
        ));
    }
    // The shell has already moved, so rediscover from the stored common dir rather than the cwd.
    let repository = discover_repository_from_common_dir(&operation.repository_common_dir)?;
    let state = load_repository_state(&config, &repository)?;
    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == operation.workspace_id)
        .cloned()
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "The workspace disappeared before Acre could finish returning it.",
                exit::CONFLICT,
            )
        })?;
    let shell_directory = std::env::current_dir()?;
    if is_inside(&workspace.path, &context.cwd) || is_inside(&workspace.path, &shell_directory) {
        return Err(AcreError::new(
            "ACRE_SHELL_STILL_INSIDE",
            "Move the shell out of the workspace before resuming this operation.",
            exit::REFUSED,
        ));
    }
    let registered = repository.worktree_at(&workspace.path);
    let status = read_status(&workspace.path)?;
    // Anything that happened between the two phases voids the earlier assessment.
    if registered.is_none_or(|worktree| worktree.head != operation.expected_head)
        || status.fingerprint != operation.expected_status_fingerprint
    {
        remove_pending_operation(&config, token)?;
        record_left_workspace(
            context,
            &config,
            &repository,
            &operation,
            &workspace,
            &shell_directory,
            false,
        )?;
        return Err(AcreError::new(
            "ACRE_WORKSPACE_CHANGED",
            "The workspace changed while Acre was moving the shell. Acre left it active.",
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "workspace": workspace.path })));
    }
    let result = return_workspace(
        &config,
        &repository,
        &workspace.id,
        &assess_options(context, operation.current_session_id.clone()),
    );
    remove_pending_operation(&config, token)?;
    let returned = result.as_ref().is_ok_and(|result| result.returned);
    record_left_workspace(
        context,
        &config,
        &repository,
        &operation,
        &workspace,
        &shell_directory,
        returned,
    )?;
    Ok(render_return(context, &workspace, &result?))
}

/// Records the shell's move out of the workspace. A returned workspace is a pooled slot now, so
/// it must not become the place `acre -` leads back to.
fn record_left_workspace(
    context: &CommandContext,
    config: &AcreConfig,
    repository: &Repository,
    operation: &PendingDoneOperation,
    workspace: &WorkspaceRecord,
    shell_directory: &Path,
    returned: bool,
) -> Result<()> {
    let from = if returned {
        shell_directory
    } else {
        operation.original_directory.as_deref().unwrap_or(&workspace.path)
    };
    record_shell_move(
        context,
        config,
        from,
        shell_directory,
        repository_at(shell_directory, repository).as_ref(),
        false,
    )
}

fn safe_destination(
    context: &CommandContext,
    config: &AcreConfig,
    repository: &Repository,
    leaving_path: &Path,
    shell_directory: &Path,
) -> Result<PathBuf> {
    if let Some(session_id) = &context.shell.session_id
        && let Some(previous) =
            read_shell_state(config, session_id)?.and_then(|state| state.previous_directory)
    {
        // The previous location is only safe if it isn't inside the directory we're about to remove.
        if !is_inside(leaving_path, &previous) && previous.is_dir() {
            return Ok(previous);
        }
    }
    let primary = repository
        // Fall back to the primary worktree; it is permanent, so it is always a safe place to stand.
        .primary_worktree()
        .filter(|worktree| {
            canonical_or_absolute(&worktree.path) != canonical_or_absolute(leaving_path)
                && worktree.path.exists()
        })
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_NO_SAFE_DESTINATION",
                "Acre could not find another worktree to move this shell into.",
                exit::REFUSED,
            )
        })?;
    // Keep the relative position: leaving src/api lands in the primary's src/api when it exists.
    // Compared canonically: the shell reports resolved paths, stored paths may go through symlinks.
    let candidate = canonical_or_absolute(shell_directory)
        .strip_prefix(canonical_or_absolute(leaving_path))
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| primary.path.join(relative));
    Ok(candidate
        .filter(|candidate| candidate.is_dir())
        .unwrap_or_else(|| primary.path.clone()))
}

/// Reports how a return went and gives the exit code for it.
fn render_return(context: &CommandContext, workspace: &WorkspaceRecord, result: &ReturnResult) -> i32 {
    if result.returned {
        render_done(context, workspace, result);
        exit::SUCCESS
    } else {
        render_unsafe(context, &result.assessment);
        exit::REFUSED
    }
}

fn render_unsafe(context: &CommandContext, assessment: &DoneAssessment) {
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": false,
            "retained": true,
            "workspace": assessment.workspace,
            "assessment": assessment,
        }));
        return;
    }
    renderer.line(format!(
        "<bold><yellow>{} can't be returned yet</yellow></bold>",
        renderer.value(&assessment.workspace.target.display_name)
    ));
    renderer.line("");
    if assessment.status.staged > 0 {
        renderer.line(format!("  {} staged", assessment.status.staged));
    }
    if assessment.status.modified > 0 {
        renderer.line(format!("  {} modified", assessment.status.modified));
    }
    if assessment.status.untracked > 0 {
        renderer.line(format!("  {} untracked", assessment.status.untracked));
    }
    if let Some(operation) = &assessment.operation {
        renderer.line(format!("  {} in progress", renderer.value(operation)));
    }
    if assessment.locked {
        renderer.line(match &assessment.lock_reason {
            Some(reason) => format!("  worktree locked: {}", renderer.value(reason)),
            None => "  worktree locked".to_owned(),
        });
    }
    if assessment.detached_commits {
        renderer.line("  create a branch for detached commits before returning this workspace");
    }
    render_entries(&renderer, "ignored", &assessment.new_ignored);
    if !assessment.new_ignored.is_empty() {
        renderer.line(
            "  <dim>Delete these, or list build output under environment.outputRoots in .acre.json.</dim>",
        );
    }
    render_entries(&renderer, "changed local file", &assessment.changed_seed_files);
    if let Some(error) = &assessment.process_check_error {
        renderer.line(format!("  process check failed: {}", renderer.value(error)));
    }
    for lease in &assessment.leases {
        renderer.line(format!("  held by {}", renderer.value(&lease.holder)));
    }
    for process in &assessment.processes {
        renderer.line(format!(
            "  {} (pid {})",
            renderer.value(&process.command),
            process.pid
        ));
    }
    renderer.line("");
    renderer.line("<dim>Acre left the workspace exactly where it is.</dim>");
}

fn render_entries(renderer: &Renderer, label: &str, entries: &[String]) {
    const SHOWN: usize = 8;
    for entry in entries.iter().take(SHOWN) {
        renderer.line(format!("  {label}: {}", renderer.value(entry)));
    }
    if entries.len() > SHOWN {
        renderer.line(format!("  <dim>and {} more</dim>", entries.len() - SHOWN));
    }
}

fn render_done(context: &CommandContext, workspace: &WorkspaceRecord, result: &ReturnResult) {
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": true,
            "action": "done",
            "target": workspace.target.display_name,
            "branch": workspace.target.local_branch,
            "pooled": result.pooled,
            "external": result.external,
            "clearedOutputs": result.cleared_outputs,
        }));
        return;
    }
    renderer.line(format!(
        "<bold><green>Finished with</green></bold> <blue>{}</blue>",
        renderer.value(&workspace.target.display_name)
    ));
    renderer.line("");
    if result.external {
        renderer.line("<dim>External worktree left untouched</dim>");
    } else {
        renderer.line("<dim>Branch and commits preserved</dim>");
        if let Some(cleared) = summarize(&result.cleared_outputs) {
            renderer.line(format!(
                "<dim>Build output cleared: {}</dim>",
                renderer.value(&cleared)
            ));
        }
        renderer.line(if result.pooled {
            "<dim>Warm workspace returned to the pool</dim>"
        } else {
            "<dim>Workspace directory removed</dim>"
        });
    }
}

/// The first few paths and a count of the rest, for a single line.
fn summarize(paths: &[String]) -> Option<String> {
    const SHOWN: usize = 3;
    let (first, rest) = paths.split_at(paths.len().min(SHOWN));
    let mut summary = first.join(", ");
    if !rest.is_empty() {
        summary.push_str(&format!(" and {} more", rest.len()));
    }
    (!first.is_empty()).then_some(summary)
}
