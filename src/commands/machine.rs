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
use crate::workspace::release::return_workspace;
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
                // No shell session: machine leases are released explicitly, never by navigation.
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
        // Path on stdout, everything else on stderr, so scripts can capture the path alone.
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
    // Reported only if the lease turns up nowhere: it may be in the repository we couldn't read.
    let mut unreadable = None;
    // A lease id says nothing about its repository, so search every one we've seen.
    for known in load_repository_index(&config)? {
        let Ok(repository) = discover_repository_from_common_dir(&known.common_dir) else {
            continue;
        };
        let state = match load_repository_state(&config, &repository) {
            Ok(state) => state,
            Err(error) => {
                unreadable.get_or_insert(error);
                continue;
            }
        };
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
        let mut external = false;
        let mut assessment = None;
        let mut return_error = None;
        // The last lease out tries to return the workspace; others just let go.
        if !keep_active && remaining == 0 {
            let returned = return_workspace(
                &config,
                &repository,
                &workspace.id,
                &AssessOptions {
                    allowed_session_id: None,
                    ignored_pids: vec![std::process::id()],
                },
            );
            // The lease is already gone, so a failed return is a retained workspace, not a failed release.
            match returned {
                Ok(result) => {
                    retained = !result.returned;
                    pooled = result.pooled;
                    external = result.external;
                    assessment = Some(result.assessment);
                }
                Err(error) => return_error = Some(error),
            }
        }
        let renderer = Renderer::new(context);
        if context.global.json {
            renderer.json(&serde_json::json!({
                "ok": true,
                "lease_id": lease_id,
                "workspace": workspace.target.display_name,
                "retained": retained,
                "pooled": pooled,
                "external": external,
                "assessment": if retained { assessment.as_ref() } else { None },
                "return_error": return_error.as_ref().map(|error| serde_json::json!({
                    "code": error.code,
                    "message": error.message,
                })),
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
            } else if external {
                renderer.line("<dim>External worktree left untouched</dim>");
            } else {
                renderer.line(
                    "<dim>Workspace remains active because Acre could not prove it safe to recycle</dim>",
                );
                let reasons = assessment
                    .iter()
                    .flat_map(|assessment| assessment.reasons.iter().cloned())
                    .chain(return_error.iter().map(ToString::to_string));
                for reason in reasons {
                    renderer.line(format!("  <yellow>•</yellow> {}", renderer.value(reason)));
                }
            }
        }
        return Ok(exit::SUCCESS);
    }
    Err(unreadable.unwrap_or_else(|| {
        AcreError::new(
            "ACRE_LEASE_NOT_FOUND",
            "That Acre lease was not found.",
            exit::NOT_FOUND,
        )
        .with_details(serde_json::json!({ "leaseId": lease_id }))
    }))
}
