//! Hidden commands used by the shell integration and the background replenisher.

use crate::cli::CommandContext;
use crate::commands::done;
use crate::error::{Result, exit};
use crate::git::refs::list_refs;
use crate::git::repository::{discover_repository, discover_repository_from_common_dir};
use crate::state::config::load_config;
use crate::state::shell::new_shell_session_id;
use crate::ui::output::Renderer;
use crate::workspace::pool::warm_repository;

pub fn session_id(context: &CommandContext) -> Result<i32> {
    let id = new_shell_session_id();
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "session_id": id}));
    } else {
        renderer.raw(format!("{id}\n"));
    }
    Ok(exit::SUCCESS)
}

pub fn resume(context: &CommandContext, token: &str) -> Result<i32> {
    done::resume(context, token)
}

pub fn replenish(context: &CommandContext, common_dir: &std::path::Path) -> Result<i32> {
    let result = (|| -> Result<()> {
        let config = load_config()?;
        let repository = discover_repository_from_common_dir(common_dir)?;
        warm_repository(&config, &repository, Some(config.pool.min_slots))?;
        Ok(())
    })();
    // Detached from any terminal: a failure here has nobody to report to.
    if std::env::var_os("ACRE_BACKGROUND").is_some() {
        return Ok(exit::SUCCESS);
    }
    result?;
    if context.global.json {
        Renderer::new(context).json(&serde_json::json!({"ok": true}));
    }
    Ok(exit::SUCCESS)
}

pub fn complete(context: &CommandContext, token: &str) -> Result<i32> {
    // Subcommands a person types; hidden ones stay out of completion.
    let mut values = ["new", "done", "setup", "-", "pr:"]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    if let Ok(repository) = discover_repository(&context.cwd) {
        for worktree in &repository.worktrees {
            if let Some(branch) = &worktree.branch {
                values.insert(branch.clone());
            }
        }
        if let Ok(refs) = list_refs(&repository.top_level) {
            for reference in refs {
                values.insert(reference.qualified_name());
            }
        }
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
