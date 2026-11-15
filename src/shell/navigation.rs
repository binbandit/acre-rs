//! Moves the parent shell: chooses the matching directory inside a target and writes the directive.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::{Repository, discover_repository};
use crate::model::AcreConfig;
use crate::shell::directive::write_cd_directive;
use crate::state::shell::{read_shell_state, record_navigation};
use crate::ui::output::Renderer;
use crate::util::{display_path, is_inside};
use crate::workspace::lease::{LeaseRequest, lease_workspace_by_path, release_shell_session_lease};

pub fn navigation_destination(
    context: &CommandContext,
    repository: &Repository,
    target_root: &Path,
) -> PathBuf {
    let Some(current) = &repository.current_worktree else {
        return target_root.to_path_buf();
    };
    if !is_inside(&current.path, &context.cwd) {
        return target_root.to_path_buf();
    }
    let Ok(relative) = context.cwd.strip_prefix(&current.path) else {
        return target_root.to_path_buf();
    };
    if relative.as_os_str().is_empty() {
        return target_root.to_path_buf();
    }
    // Same relative spot in the new worktree, when it exists there.
    let candidate = target_root.join(relative);
    if candidate.is_dir() {
        candidate
    } else {
        target_root.to_path_buf()
    }
}

/// Moves the shell to `destination`, which lies in `repository` when it is inside one at all.
pub fn navigate_direct(
    context: &CommandContext,
    config: &AcreConfig,
    destination: &Path,
    repository: Option<&Repository>,
    label: &str,
) -> Result<()> {
    if !context.shell.active {
        return Err(AcreError::new(
            "ACRE_SHELL_INTEGRATION_REQUIRED",
            "Acre cannot move this shell until shell integration is installed.",
            exit::ENVIRONMENT,
        )
        .with_details(serde_json::json!({
            "destination": destination,
            "hint": "acre setup",
        })));
    }
    record_shell_move(
        context,
        config,
        &shell_directory(context),
        destination,
        repository,
        false,
    )?;
    let renderer = Renderer::new(context);
    renderer.line(format!(
        "<green>→</green> <bold><blue>{}</blue></bold>",
        renderer.value(label)
    ));
    // Only spell out the path when the label alone doesn't say where we went.
    if destination.file_name().and_then(|value| value.to_str()) != Some(label) {
        renderer.line(format!(
            "<dim>{}</dim>",
            renderer.value(display_path(destination))
        ));
    }
    write_cd_directive(context, destination)
}

/// Where the calling shell stands. A directory deleted out from under it has no current path, so
/// fall back to where the command was aimed rather than failing after the summary was printed.
pub fn shell_directory(context: &CommandContext) -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| context.cwd.clone())
}

/// The repository holding `path`: `known` when the path is inside it, otherwise whatever Git
/// finds there, which is nothing for a plain directory.
pub fn repository_at(path: &Path, known: &Repository) -> Option<Repository> {
    if known.contains(path) {
        Some(known.clone())
    } else {
        discover_repository(path).ok()
    }
}

/// Records the shell's move in its history and moves its lease along: released in a repository
/// it leaves, taken on the workspace it lands in. `destination` is the repository holding `to`.
///
/// Lease updates are best effort so navigation stays responsive while another operation holds a
/// repository lock; history is always written.
pub fn record_shell_move(
    context: &CommandContext,
    config: &AcreConfig,
    from: &Path,
    to: &Path,
    destination: Option<&Repository>,
    destination_leased: bool,
) -> Result<()> {
    let Some(session) = context
        .shell
        .session_id
        .as_deref()
        .filter(|_| context.shell.active)
    else {
        return Ok(());
    };
    // The session's lease may sit where Acre last put the shell rather than where it stands now.
    let recorded = read_shell_state(config, session)?.and_then(|state| state.current_directory);
    let mut released = BTreeSet::new();
    for source in recorded.as_deref().into_iter().chain([from]) {
        // Moving within one repository keeps the lease there; it is replaced below.
        if destination.is_some_and(|repository| repository.contains(source)) {
            continue;
        }
        if let Ok(repository) = discover_repository(source)
            && destination.is_none_or(|target| target.common_dir != repository.common_dir)
            && released.insert(repository.common_dir.clone())
        {
            let _ = release_shell_session_lease(config, &repository, session);
        }
    }
    if !destination_leased
        && let (Some(repository), Some(request)) = (destination, LeaseRequest::for_shell(&context.shell))
    {
        let _ = lease_workspace_by_path(config, repository, to, &request);
    }
    record_navigation(config, session, from, to, context.shell.pid)
}
