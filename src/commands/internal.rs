use crate::commands::done::command_resume_done;
use crate::error::{Result, exit};
use crate::git::refs::list_refs;
use crate::git::repository::{discover_repository, discover_repository_from_common_dir};
use crate::model::CommandContext;
use crate::pool::broker::warm_repository;
use crate::state::config::load_config;
use crate::state::shell::new_shell_session_id;
use crate::ui::output::Renderer;

pub fn command_session_id(context: &CommandContext) -> Result<i32> {
    Renderer::new(context).raw(format!("{}\n", new_shell_session_id()));
    Ok(exit::SUCCESS)
}

pub fn command_resume(context: &CommandContext, token: &str) -> Result<i32> {
    command_resume_done(context, token)
}

pub fn command_replenish(common_dir: &std::path::Path) -> Result<i32> {
    let result = (|| -> Result<()> {
        let config = load_config()?;
        let repository = discover_repository_from_common_dir(common_dir)?;
        warm_repository(&config, &repository, Some(config.pool.min_slots))?;
        Ok(())
    })();
    if std::env::var_os("ACRE_BACKGROUND").is_some() {
        return Ok(exit::SUCCESS);
    }
    result.map(|()| exit::SUCCESS)
}

pub fn command_complete(context: &CommandContext, token: &str) -> Result<i32> {
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
                values.insert(match reference.kind {
                    crate::model::GitRefKind::Local => reference.short_name,
                    crate::model::GitRefKind::Remote => format!(
                        "{}/{}",
                        reference.remote.unwrap_or_default(),
                        reference.short_name
                    ),
                });
            }
        }
    }
    let lower = token.to_ascii_lowercase();
    let output = values
        .into_iter()
        .filter(|value| token.is_empty() || value.to_ascii_lowercase().starts_with(&lower))
        .collect::<Vec<_>>()
        .join("\n");
    Renderer::new(context).raw(format!("{output}\n"));
    Ok(exit::SUCCESS)
}
