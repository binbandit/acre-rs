//! Hidden commands used by the shell integration and by background pool maintenance.

use std::collections::BTreeSet;
use std::path::Path;

use crate::cli::CommandContext;
use crate::error::{Result, exit};
use crate::git::refs::list_refs;
use crate::git::repository::discover_repository_from_common_dir;
use crate::state::config::load_config;
use crate::ui::output::Renderer;
use crate::util::random_id;
use crate::workspace::maintain::empty_trash;
use crate::workspace::pool::warm_repository;

pub fn session_id(context: &CommandContext) -> Result<i32> {
    let id = random_id();
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "session_id": id}));
    } else {
        renderer.raw(format!("{id}\n"));
    }
    Ok(exit::SUCCESS)
}

pub fn replenish(context: &CommandContext, common_dir: &Path) -> Result<i32> {
    maintenance(context, || {
        let config = load_config()?;
        let repository = discover_repository_from_common_dir(common_dir)?;
        warm_repository(&config, &repository, Some(config.pool.min_slots))?;
        Ok(())
    })
}

pub fn purge(context: &CommandContext, common_dir: &Path) -> Result<i32> {
    maintenance(context, || {
        let config = load_config()?;
        let repository = discover_repository_from_common_dir(common_dir)?;
        empty_trash(&config, &repository)
    })
}

/// Runs pool maintenance that is normally launched detached from any terminal, where a failure
/// has nobody to report to.
fn maintenance(context: &CommandContext, work: impl FnOnce() -> Result<()>) -> Result<i32> {
    let result = work();
    if std::env::var_os("ACRE_BACKGROUND").is_none() {
        result?;
        if context.global.json {
            Renderer::new(context).json(&serde_json::json!({"ok": true}));
        }
    }
    Ok(exit::SUCCESS)
}

pub fn complete(context: &CommandContext, token: &str) -> Result<i32> {
    // Subcommands a person types; hidden ones stay out of completion.
    let mut values = ["new", "done", "clean", "setup", "-", "pr:"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    // Every keystroke of a completion waits on this, so it asks Git for branch names and nothing
    // else. Outside a repository there are simply none to offer.
    if let Ok(refs) = list_refs(&context.cwd) {
        values.extend(refs.iter().map(|reference| reference.qualified_name()));
    }
    let lower = token.to_ascii_lowercase();
    let output = values
        .into_iter()
        .filter(|value| token.is_empty() || value.to_ascii_lowercase().starts_with(&lower))
        .collect::<Vec<_>>();
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "candidates": output}));
    } else {
        renderer.raw(format!("{}\n", output.join("\n")));
    }
    Ok(exit::SUCCESS)
}
