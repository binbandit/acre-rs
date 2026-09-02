//! Moves the parent shell: chooses the matching directory inside a target and writes the directive.

use std::path::{Path, PathBuf};

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::Repository;
use crate::model::AcreConfig;
use crate::shell::directive::write_cd_directive;
use crate::state::shell::record_navigation;
use crate::ui::output::Renderer;
use crate::util::{display_path, is_inside};

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
    let candidate = target_root.join(relative);
    if candidate.is_dir() {
        candidate
    } else {
        target_root.to_path_buf()
    }
}

pub fn navigate_direct(
    context: &CommandContext,
    config: &AcreConfig,
    destination: &Path,
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
    let session_id = context.shell.session_id.as_deref().expect("checked above");
    record_navigation(config, session_id, &context.cwd, destination, context.shell.pid)?;
    let renderer = Renderer::new(context);
    renderer.line(format!(
        "<green>→</green> <bold><blue>{}</blue></bold>",
        renderer.value(label)
    ));
    if destination.file_name().and_then(|value| value.to_str()) != Some(label) {
        renderer.line(format!(
            "<dim>{}</dim>",
            renderer.value(display_path(destination))
        ));
    }
    write_cd_directive(context, destination)
}
