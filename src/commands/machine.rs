//! `acre acquire` and `acre release`: the lease-based API for editors and agents.

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::{discover_repository, discover_repository_from_common_dir};
use crate::model::EnvironmentState;
use crate::state::config::load_config;
use crate::state::index::load_repository_index;
use crate::state::repository::load_repository_state;
use crate::ui::output::Renderer;
use crate::workspace::activate::{MaterializeOptions, materialize_workspace};
use crate::workspace::assess::AssessOptions;
use crate::workspace::lease::{LeaseRequest, release_workspace_lease};
use crate::workspace::release::{ReturnOptions, return_workspace};
use crate::workspace::resolve::{resolve_existing_target, resolve_new_target};

pub fn acquire(
    context: &CommandContext,
    selector: &str,
    holder: &str,
    pid: Option<u32>,
    new: bool,
    from: Option<&str>,
    fresh: bool,
) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    let target = if new {
        resolve_new_target(&repository, selector, from, fresh)?
    } else {
        resolve_existing_target(&repository, &state, selector)?
    };
    let result = materialize_workspace(
        &config,
        &repository,
        &target,
        &MaterializeOptions {
            lease: Some(LeaseRequest {
                holder: holder.to_owned(),
                pid,
                session_id: None,
            }),
            no_replenish: false,
        },
    )?;
    let renderer = Renderer::new(context);
    let payload = serde_json::json!({
        "ok": true,
        "path": result.path,
        "branch": result.target.local_branch,
        "target": result.target.display_name,
        "target_kind": result.target.kind,
        "lease_id": result.lease.as_ref().map(|lease| lease.id.as_str()),
        "ownership": result.workspace.ownership,
        "environment": result.workspace.environment,
        "reused": result.reused,
        "elapsed_ms": result.elapsed_ms,
    });
    if context.global.json {
        renderer.json(&payload);
    } else {
        renderer.raw(format!("{}\n", result.path.display()));
        renderer.error(format!(
            "<dim>lease {} · {}</dim>",
            renderer.value(
                result
                    .lease
                    .as_ref()
                    .map(|lease| lease.id.as_str())
                    .unwrap_or("none")
            ),
            result
                .workspace
                .environment
                .as_ref()
                .map(|environment| environment.state)
                .unwrap_or(EnvironmentState::Unknown),
        ));
    }
    Ok(exit::SUCCESS)
}

pub fn release(context: &CommandContext, lease_id: &str, keep_active: bool) -> Result<i32> {
    let config = load_config()?;
    for known in load_repository_index(&config)? {
        let Ok(repository) = discover_repository_from_common_dir(&known.common_dir) else {
            continue;
        };
        let state = load_repository_state(&config, &repository)?;
        if !state.leases.iter().any(|lease| lease.id == lease_id) {
            continue;
        }
        let (_lease, released_state, workspace) = release_workspace_lease(&config, &repository, lease_id)?;
        let remaining = released_state
            .leases
            .iter()
            .filter(|lease| lease.workspace_id == workspace.id)
            .count();
        let mut retained = true;
        let mut pooled = false;
        let mut assessment = None;
        if !keep_active && remaining == 0 {
            let result = return_workspace(
                &config,
                &repository,
                &workspace.id,
                &ReturnOptions {
                    assessment: AssessOptions {
                        allowed_session_id: None,
                        allowed_lease_id: None,
                        ignored_pids: vec![std::process::id()],
                    },
                    remove_lease_id: None,
                },
            )?;
            retained = !result.returned;
            pooled = result.pooled;
            assessment = Some(result.assessment);
        }
        let renderer = Renderer::new(context);
        if context.global.json {
            renderer.json(&serde_json::json!({
                "ok": true,
                "lease_id": lease_id,
                "workspace": workspace.target.display_name,
                "retained": retained,
                "pooled": pooled,
                "assessment": if retained { assessment.as_ref() } else { None },
            }));
        } else {
            renderer.line(format!(
                "<bold><green>Released</green></bold> <blue>{}</blue>",
                renderer.value(&workspace.target.display_name)
            ));
            if !retained {
                renderer.line(if pooled {
                    "<dim>Warm workspace returned to the pool</dim>"
                } else {
                    "<dim>Workspace directory removed</dim>"
                });
            } else if keep_active {
                renderer.line("<dim>Workspace remains active by request</dim>");
            } else if remaining > 0 {
                renderer.line(format!(
                    "<dim>Workspace remains active · {remaining} other lease{}</dim>",
                    if remaining == 1 { "" } else { "s" }
                ));
            } else {
                renderer.line(
                    "<dim>Workspace remains active because Acre could not prove it safe to recycle</dim>",
                );
                if let Some(assessment) = &assessment {
                    for reason in &assessment.reasons {
                        renderer.line(format!("  <yellow>•</yellow> {}", renderer.value(reason)));
                    }
                }
            }
        }
        return Ok(exit::SUCCESS);
    }
    Err(AcreError::new(
        "ACRE_LEASE_NOT_FOUND",
        "That Acre lease was not found.",
        exit::NOT_FOUND,
    )
    .with_details(serde_json::json!({ "leaseId": lease_id })))
}
