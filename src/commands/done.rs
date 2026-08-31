use std::path::{Path, PathBuf};

use crate::error::{AcreError, Result, exit};
use crate::git::repository::{discover_repository, discover_repository_from_common_dir};
use crate::git::status::read_status;
use crate::model::{CommandContext, PendingDoneOperation, Repository, WorkspaceOwnership, WorkspaceRecord};
use crate::pool::assessment::AssessOptions;
use crate::pool::broker::{
    ReturnOptions, assess_workspace_for_return, release_shell_session_lease, return_workspace,
};
use crate::shell::directive::write_resume_directive;
use crate::shell::navigation::navigate_direct;
use crate::state::config::load_config;
use crate::state::operations::{read_pending_done, remove_pending_operation, save_pending_done};
use crate::state::repository::load_repository_state;
use crate::state::shell::read_shell_state;
use crate::target::resolve_existing_target;
use crate::ui::output::Renderer;
use crate::util::{canonical_or_absolute, is_inside, now_iso, random_id};

pub fn command_done(context: &CommandContext, selector: Option<&str>) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    let current_path = repository.current_worktree.as_ref().map(|worktree| worktree.path.clone());

    let target_path = if let Some(selector) = selector {
        let target = resolve_existing_target(&repository, &state, selector)?;
        target
            .existing_worktree
            .map(|worktree| worktree.path)
            .or_else(|| {
                state
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.target.local_branch == target.local_branch)
                    .map(|workspace| workspace.path.clone())
            })
            .ok_or_else(|| {
                AcreError::new(
                    "ACRE_WORKSPACE_NOT_ACTIVE",
                    format!("{selector} does not currently have a workspace."),
                    exit::NOT_FOUND,
                )
            })?
    } else {
        current_path.clone().ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "Acre could not identify the current worktree.",
                exit::NOT_FOUND,
            )
        })?
    };

    let workspace = state
        .workspaces
        .iter()
        .find(|workspace| canonical_or_absolute(&workspace.path) == canonical_or_absolute(&target_path))
        .cloned();
    if repository
        .worktrees
        .iter()
        .find(|worktree| canonical_or_absolute(&worktree.path) == canonical_or_absolute(&target_path))
        .is_some_and(|worktree| worktree.is_main)
    {
        return Err(AcreError::new(
            "ACRE_PRIMARY_WORKTREE",
            "The repository primary worktree is permanent; Acre only returns workspaces that it created.",
            exit::REFUSED,
        )
        .with_details(serde_json::json!({ "path": target_path })));
    }
    let is_current = current_path.as_ref().is_some_and(|current| canonical_or_absolute(current) == canonical_or_absolute(&target_path))
        && is_inside(&target_path, &context.cwd);

    if workspace.as_ref().is_none_or(|workspace| workspace.ownership == WorkspaceOwnership::External) {
        let renderer = Renderer::new(context);
        if context.global.json {
            renderer.json(&serde_json::json!({
                "ok": true,
                "action": "done",
                "external": true,
                "retained": true,
                "path": target_path,
            }));
            return Ok(exit::SUCCESS);
        }
        if !is_current {
            renderer.line(format!("<dim>External worktree {} was left untouched.</dim>", renderer.value(selector.unwrap_or_else(|| target_path.to_str().unwrap_or("worktree")))));
            return Ok(exit::SUCCESS);
        }
        let destination = safe_destination(context, &config, &repository, &target_path)?;
        if let Some(session_id) = &context.shell.session_id {
            let _ = release_shell_session_lease(&config, &repository, session_id);
        }
        renderer.line(format!("<dim>External worktree {} was left untouched.</dim>", renderer.value(target_path.file_name().and_then(|value| value.to_str()).unwrap_or("worktree"))));
        let label = destination.file_name().and_then(|value| value.to_str()).unwrap_or("workspace");
        navigate_direct(context, &config, &destination, label)?;
        return Ok(exit::SUCCESS);
    }
    let workspace = workspace.expect("checked above");
    let assess = AssessOptions {
        allowed_session_id: context.shell.session_id.clone(),
        allowed_lease_id: None,
        ignored_pids: [Some(std::process::id()), context.shell.pid].into_iter().flatten().collect(),
    };
    let assessment = assess_workspace_for_return(&config, &repository, &workspace.id, &assess)?;
    if !assessment.safe {
        render_unsafe(context, &assessment);
        return Ok(exit::REFUSED);
    }

    if !is_current {
        let result = return_workspace(
            &config,
            &repository,
            &workspace.id,
            &ReturnOptions { assessment: assess, remove_lease_id: None },
        )?;
        render_done(context, &workspace, result.pooled, result.external);
        return Ok(if result.returned || result.external { exit::SUCCESS } else { exit::REFUSED });
    }

    if !context.shell.active || context.shell.session_id.is_none() {
        return Err(AcreError::new(
            "ACRE_SHELL_INTEGRATION_REQUIRED",
            "Acre cannot safely return the workspace containing this shell until shell integration is installed.",
            exit::ENVIRONMENT,
        )
        .with_details(serde_json::json!({ "hint": "acre setup", "workspace": workspace.path })));
    }
    let destination = safe_destination(context, &config, &repository, &workspace.path)?;
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
        })?;
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
            current_session_id: context.shell.session_id.clone(),
            created_at: now_iso(),
        },
    )?;
    write_resume_directive(context, &destination, &token)?;
    Ok(exit::RESUME)
}

pub fn command_resume_done(context: &CommandContext, token: &str) -> Result<i32> {
    let config = load_config()?;
    let operation = read_pending_done(&config, token)?.ok_or_else(|| {
        AcreError::new(
            "ACRE_OPERATION_NOT_FOUND",
            "This Acre operation has expired or already completed.",
            exit::NOT_FOUND,
        )
    })?;
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
    let registered = repository.worktrees.iter().find(|worktree| {
        canonical_or_absolute(&worktree.path) == canonical_or_absolute(&workspace.path)
    });
    let status = read_status(&workspace.path)?;
    if registered.is_none_or(|worktree| worktree.head != operation.expected_head)
        || status.fingerprint != operation.expected_status_fingerprint
    {
        remove_pending_operation(&config, token)?;
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
        &ReturnOptions {
            assessment: AssessOptions {
                allowed_session_id: operation.current_session_id,
                allowed_lease_id: None,
                ignored_pids: [Some(std::process::id()), context.shell.pid].into_iter().flatten().collect(),
            },
            remove_lease_id: None,
        },
    )?;
    remove_pending_operation(&config, token)?;
    if !result.returned {
        render_unsafe(context, &result.assessment);
        return Ok(exit::REFUSED);
    }
    render_done(context, &workspace, result.pooled, result.external);
    Ok(exit::SUCCESS)
}

fn safe_destination(
    context: &CommandContext,
    config: &crate::model::AcreConfig,
    repository: &Repository,
    leaving_path: &Path,
) -> Result<PathBuf> {
    if let Some(session_id) = &context.shell.session_id {
        if let Some(previous) = read_shell_state(config, session_id)?.and_then(|state| state.previous_directory) {
            if !is_inside(leaving_path, &previous) && previous.is_dir() {
                return Ok(previous);
            }
        }
    }
    let primary = repository
        .worktrees
        .iter()
        .find(|worktree| worktree.is_main)
        .or_else(|| repository.worktrees.first())
        .filter(|worktree| canonical_or_absolute(&worktree.path) != canonical_or_absolute(leaving_path) && worktree.path.exists())
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_NO_SAFE_DESTINATION",
                "Acre could not find another worktree to move this shell into.",
                exit::REFUSED,
            )
        })?;
    let candidate = context
        .cwd
        .strip_prefix(leaving_path)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| primary.path.join(relative));
    Ok(candidate.filter(|candidate| candidate.is_dir()).unwrap_or_else(|| primary.path.clone()))
}

fn render_unsafe(context: &CommandContext, assessment: &crate::model::DoneAssessment) {
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
    renderer.line(format!("<bold><yellow>{} is still in use</yellow></bold>", renderer.value(&assessment.workspace.target.display_name)));
    renderer.line("");
    if assessment.status.staged > 0 { renderer.line(format!("  {} staged", assessment.status.staged)); }
    if assessment.status.modified > 0 { renderer.line(format!("  {} modified", assessment.status.modified)); }
    if assessment.status.untracked > 0 { renderer.line(format!("  {} untracked", assessment.status.untracked)); }
    if let Some(operation) = &assessment.operation { renderer.line(format!("  {} in progress", renderer.value(operation))); }
    for entry in assessment.new_ignored.iter().take(8) { renderer.line(format!("  ignored: {}", renderer.value(entry))); }
    for entry in assessment.changed_seed_files.iter().take(8) { renderer.line(format!("  changed local file: {}", renderer.value(entry))); }
    for lease in &assessment.leases { renderer.line(format!("  held by {}", renderer.value(&lease.holder))); }
    for process in &assessment.processes { renderer.line(format!("  {} (pid {})", renderer.value(&process.command), process.pid)); }
    renderer.line("");
    renderer.line("<dim>Acre left the workspace exactly where it is.</dim>");
}

fn render_done(context: &CommandContext, workspace: &WorkspaceRecord, pooled: bool, external: bool) {
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": true,
            "action": "done",
            "target": workspace.target.display_name,
            "branch": workspace.target.local_branch,
            "pooled": pooled,
            "external": external,
        }));
        return;
    }
    renderer.line(format!("<bold><green>Finished with</green></bold> <blue>{}</blue>", renderer.value(&workspace.target.display_name)));
    renderer.line("");
    if external {
        renderer.line("<dim>External worktree left untouched</dim>");
    } else {
        renderer.line("<dim>Branch and commits preserved</dim>");
        renderer.line(if pooled { "<dim>Warm workspace returned to the pool</dim>" } else { "<dim>Workspace directory removed</dim>" });
    }
}
