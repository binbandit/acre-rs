//! What happens after a workspace is opened: report it, then move the shell into it.

use std::path::{Path, PathBuf};

use crate::cli::CommandContext;
use crate::error::Result;
use crate::model::{AcreConfig, EnvironmentState, TargetKind, TrustLevel};
use crate::shell::directive::write_cd_directive;
use crate::shell::navigation::navigation_destination;
use crate::state::shell::record_navigation;
use crate::ui::output::Renderer;
use crate::util::{display_path, shell_quote};
use crate::workspace::activate::MaterializedWorkspace;

pub fn navigate_to_materialized(
    context: &CommandContext,
    config: &AcreConfig,
    result: &MaterializedWorkspace,
) -> Result<PathBuf> {
    // Land in the same subdirectory the user was in, when the new worktree has it.
    let destination = navigation_destination(context, &result.repository, &result.path);
    let renderer = Renderer::new(context);
    if context.global.json {
        // JSON mode never writes a directive, so the payload must not claim the shell moved.
        renderer.json(&materialized_payload(result, &destination, false));
        return Ok(destination);
    }
    render_materialized_summary(&renderer, result, &destination, true);
    // Without the wrapper we can't move the shell; print the cd instead of failing.
    if !context.shell.active {
        renderer.line("");
        renderer.line("Move this shell:");
        renderer.line(format!(
            "  <blue>cd {}</blue>",
            renderer.value(shell_quote(&destination))
        ));
        renderer.line("");
        renderer.line("Enable direct navigation with <blue>acre setup</blue>.");
        return Ok(destination);
    }
    let session_id = context.shell.session_id.as_deref().expect("checked above");
    record_navigation(config, session_id, &context.cwd, &destination, context.shell.pid)?;
    write_cd_directive(context, &destination)?;
    Ok(destination)
}

pub fn materialized_payload(
    result: &MaterializedWorkspace,
    destination: &Path,
    navigated: bool,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "action": if result.created { "materialized" } else { "opened" },
        "path": destination,
        "workspace_path": result.path,
        "branch": result.target.local_branch,
        "target": result.target.display_name,
        "target_kind": result.target.kind,
        "ownership": result.workspace.ownership,
        "navigated": navigated,
        "reused": result.reused,
        "elapsed_ms": result.elapsed_ms,
        "base_ref": result.target.base_ref,
        "environment": result.workspace.environment,
    })
}
pub fn render_materialized_summary(
    renderer: &Renderer,
    result: &MaterializedWorkspace,
    destination: &Path,
    navigated: bool,
) {
    match result.target.kind {
        TargetKind::NewBranch => {
            renderer.line(format!(
                "<bold><green>Created</green></bold> <blue>{}</blue>",
                renderer.value(&result.target.display_name)
            ));
            if let Some(base) = &result.target.base_ref {
                renderer.line(format!("<dim>from {}</dim>", renderer.value(base)));
            }
            renderer.line("");
        }
        TargetKind::RemoteBranch => {
            if let Some(remote) = &result.target.remote_branch {
                renderer.line(format!(
                    "<dim>Tracking</dim> <blue>{}</blue>",
                    renderer.value(remote)
                ));
            }
        }
        TargetKind::PullRequest => {
            if let Some(pull_request) = &result.target.pull_request {
                renderer.line(format!("<dim>Opening PR #{}</dim>", pull_request.number));
            }
        }
        _ => {}
    }
    if navigated {
        renderer.line(format!(
            "<green>→</green> <bold><blue>{}</blue></bold>",
            renderer.value(&result.target.display_name)
        ));
    } else {
        renderer.line(format!(
            "<bold>{}</bold>",
            renderer.value(environment_label(result))
        ));
        renderer.line(format!(
            "<dim>{}</dim>",
            renderer.value(display_path(&result.path))
        ));
    }
    if navigated {
        let mut details = Vec::new();
        if let Ok(relative) = destination.strip_prefix(&result.path) {
            if !relative.as_os_str().is_empty() {
                details.push(relative.display().to_string());
            }
        }
        details.push(environment_label(result));
        details.push(format!("{} ms", result.elapsed_ms.max(1)));
        renderer.line(format!("<dim>{}</dim>", renderer.value(details.join(" · "))));
    } else {
        renderer.line(format!("<dim>{} ms</dim>", result.elapsed_ms.max(1)));
    }
    if result.target.trust == TrustLevel::Untrusted {
        renderer.line("<yellow>Fork secrets withheld · no repository code was run</yellow>");
    }
    if result
        .workspace
        .environment
        .as_ref()
        // Cold is the one state that needs the user to act, so it gets the loudest line.
        .is_some_and(|environment| environment.state == EnvironmentState::Cold)
    {
        renderer.line("");
        renderer.line("<yellow>No compatible prepared environment exists yet.</yellow>");
        renderer.line("<dim>Run the project’s normal install or build command. Acre will retain the clean result for matching branches.</dim>");
    }
}
fn environment_label(result: &MaterializedWorkspace) -> String {
    let Some(environment) = &result.workspace.environment else {
        return "environment unknown".to_owned();
    };
    let generation = &environment.fingerprint[..environment.fingerprint.len().min(8)];
    match (result.reused, environment.state) {
        (true, EnvironmentState::Ready) => format!("workspace ready · reused environment {generation}"),
        (false, EnvironmentState::Ready) => format!("workspace ready · environment {generation}"),
        (true, EnvironmentState::Warm) => format!("workspace warm · reused environment {generation}"),
        (false, EnvironmentState::Warm) => format!("workspace warm · environment {generation}"),
        (_, EnvironmentState::Cold) => format!("code ready · environment {generation} cold"),
        _ => format!("environment {generation} unknown"),
    }
}
