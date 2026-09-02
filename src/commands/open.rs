use std::ffi::OsString;

use crate::error::{AcreError, Result, exit};
use crate::git::refs::list_refs;
use crate::git::repository::discover_repository;
use crate::git::runner::run_passthrough;
use crate::model::{CommandContext, GitRef, GitRefKind};
use crate::pool::broker::{
    LeaseRequest, MaterializeOptions, lease_workspace_by_path, materialize_workspace,
    release_shell_session_lease, release_workspace_lease,
};
use crate::shell::navigation::{navigate_direct, navigate_to_materialized};
use crate::state::config::load_config;
use crate::state::repository::load_repository_state;
use crate::state::shell::read_shell_state;
use crate::target::resolve_existing_target;
use crate::ui::output::Renderer;
use crate::ui::picker::{PickerResult, PickerRow, pick};

pub fn command_open(
    context: &CommandContext,
    selector: Option<&str>,
    child_argv: &[OsString],
) -> Result<i32> {
    let config = load_config()?;
    if selector == Some("-") {
        if !child_argv.is_empty() {
            return Err(AcreError::new(
                "ACRE_INVALID_PREVIOUS_COMMAND",
                "Acre cannot run a command through the previous-location shortcut.",
                exit::USAGE,
            ));
        }
        let session_id = context.shell.session_id.as_deref().ok_or_else(|| {
            AcreError::new(
                "ACRE_NO_SHELL_HISTORY",
                "This shell has no Acre navigation history.",
                exit::NOT_FOUND,
            )
        })?;
        let previous = read_shell_state(&config, session_id)?
            .and_then(|state| state.previous_directory)
            .filter(|path| path.exists())
            .ok_or_else(|| {
                AcreError::new(
                    "ACRE_NO_PREVIOUS_LOCATION",
                    "The previous Acre location is no longer available.",
                    exit::NOT_FOUND,
                )
            })?;
        if let Ok(current_repository) = discover_repository(&context.cwd) {
            let _ = release_shell_session_lease(&config, &current_repository, session_id);
        }
        if let (Ok(previous_repository), Some(request)) = (
            discover_repository(&previous),
            LeaseRequest::for_shell(&context.shell),
        ) {
            let _ = lease_workspace_by_path(&config, &previous_repository, &previous, &request);
        }
        let label = previous
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("previous location");
        navigate_direct(context, &config, &previous, label)?;
        return Ok(exit::SUCCESS);
    }

    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    let selector = match selector {
        Some(selector) => selector.to_owned(),
        None => {
            if !context.interactive {
                return Err(AcreError::new(
                    "ACRE_TARGET_REQUIRED",
                    "Specify a branch, remote branch, or pull request target.",
                    exit::USAGE,
                ));
            }
            let rows = picker_rows(&repository, &state)?;
            match pick(&Renderer::new(context), &repository.name, &rows)? {
                PickerResult::Selected(value) => value,
                PickerResult::Cancelled => return Ok(exit::SUCCESS),
                PickerResult::Interrupted => return Ok(exit::INTERRUPTED),
            }
        }
    };
    let target = resolve_existing_target(&repository, &state, &selector)?;
    let lease = if child_argv.is_empty() {
        LeaseRequest::for_shell(&context.shell)
    } else {
        Some(LeaseRequest {
            holder: format!("command:{}", std::process::id()),
            pid: Some(std::process::id()),
            session_id: None,
        })
    };
    let materialized = materialize_workspace(
        &config,
        &repository,
        &target,
        &MaterializeOptions {
            lease,
            no_replenish: false,
        },
    )?;

    if !child_argv.is_empty() {
        let executable = child_argv[0].to_string_lossy().into_owned();
        let status = run_passthrough(&executable, &child_argv[1..], &materialized.path);
        if let Some(lease) = &materialized.lease {
            let _ = release_workspace_lease(&config, &materialized.repository, &lease.id);
        }
        return status;
    }
    let _ = navigate_to_materialized(context, &config, &materialized)?;
    Ok(exit::SUCCESS)
}

fn picker_rows(
    repository: &crate::model::Repository,
    state: &crate::model::RepositoryState,
) -> Result<Vec<PickerRow<String>>> {
    let mut rows = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for worktree in &repository.worktrees {
        let label = worktree
            .branch
            .clone()
            .or_else(|| {
                state
                    .workspace_at(&worktree.path)
                    .map(|workspace| workspace.target.display_name.clone())
            })
            .unwrap_or_else(|| format!("detached {}", &worktree.head[..worktree.head.len().min(8)]));
        if let Some(branch) = &worktree.branch {
            used.insert(branch.clone());
        }
        let here = repository
            .current_worktree
            .as_ref()
            .is_some_and(|current| current.path == worktree.path);
        rows.push(PickerRow {
            label: label.clone(),
            detail: Some(if here { "here · worktree" } else { "worktree" }.to_owned()),
            searchable: format!("{} {}", label, worktree.path.display()),
            value: worktree
                .branch
                .clone()
                .unwrap_or_else(|| worktree.path.display().to_string()),
        });
    }
    let refs = list_refs(&repository.top_level)?;
    for reference in refs
        .iter()
        .filter(|reference| reference.kind == GitRefKind::Local && !used.contains(&reference.short_name))
    {
        rows.push(ref_row(reference, &reference.short_name, "local branch", None));
    }
    let local_names = refs
        .iter()
        .filter(|reference| reference.kind == GitRefKind::Local)
        .map(|reference| reference.short_name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for reference in refs
        .iter()
        .filter(|reference| {
            reference.kind == GitRefKind::Remote && !local_names.contains(reference.short_name.as_str())
        })
        .take(1_000)
    {
        let remote = reference.remote.as_deref().unwrap_or("remote");
        rows.push(ref_row(
            reference,
            &reference.short_name,
            &format!("{remote} · remote branch"),
            Some(format!("{remote}/{}", reference.short_name)),
        ));
    }
    Ok(rows)
}

fn ref_row(reference: &GitRef, label: &str, detail: &str, value: Option<String>) -> PickerRow<String> {
    PickerRow {
        label: label.to_owned(),
        detail: Some(detail.to_owned()),
        searchable: format!("{label} {}", reference.full_name),
        value: value.unwrap_or_else(|| label.to_owned()),
    }
}
