//! `acre new <branch>`: create a branch and open it in a warm workspace.

use crate::cli::CommandContext;
use crate::commands::opened::{materialized_payload, navigate_to_materialized, render_materialized_summary};
use crate::error::{Result, exit};
use crate::git::repository::discover_repository;
use crate::state::config::load_config;
use crate::ui::output::Renderer;
use crate::workspace::activate::{MaterializeOptions, materialize_workspace};
use crate::workspace::lease::LeaseRequest;
use crate::workspace::resolve::resolve_new_target;

pub fn run(
    context: &CommandContext,
    branch: &str,
    from: Option<&str>,
    fresh: bool,
    stay: bool,
) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let target = resolve_new_target(&repository, branch, from, fresh)?;
    let lease = if stay {
        None
    } else {
        LeaseRequest::for_shell(&context.shell)
    };
    let result = materialize_workspace(
        &config,
        &repository,
        &target,
        &MaterializeOptions {
            lease,
            no_replenish: false,
        },
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
