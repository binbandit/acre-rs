use crate::error::{Result, exit};
use crate::git::repository::discover_repository;
use crate::model::CommandContext;
use crate::pool::broker::{LeaseRequest, MaterializeOptions, materialize_workspace};
use crate::shell::navigation::{materialized_payload, navigate_to_materialized, render_materialized_summary};
use crate::state::config::load_config;
use crate::target::resolve_new_target;
use crate::ui::output::Renderer;

pub fn command_new(
    context: &CommandContext,
    branch: &str,
    from: Option<&str>,
    fresh: bool,
    stay: bool,
) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let target = resolve_new_target(&repository, branch, from, fresh)?;
    let lease = if !stay && context.shell.active {
        context.shell.session_id.as_ref().map(|session_id| LeaseRequest {
            holder: format!("shell:{session_id}"),
            pid: context.shell.pid,
            session_id: Some(session_id.clone()),
        })
    } else {
        None
    };
    let result = materialize_workspace(
        &config,
        &repository,
        &target,
        &MaterializeOptions { lease, no_replenish: false },
    )?;
    if stay {
        let renderer = Renderer::new(context);
        if context.global.json {
            renderer.json(&materialized_payload(&result, &result.path, false));
        } else {
            render_materialized_summary(&renderer, &result, &result.path, false);
        }
    } else {
        let _ = navigate_to_materialized(context, &config, &result)?;
    }
    Ok(exit::SUCCESS)
}
