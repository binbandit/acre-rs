use crate::error::{AcreError, Result, exit};
use crate::git::repository::discover_repository;
use crate::model::{AcreConfig, CommandContext, Repository, RepositoryState, WorkspaceRecord};
use crate::state::config::load_config;
use crate::state::repository::load_repository_state;
use crate::util::canonical_or_absolute;

pub fn command_environment(context: &CommandContext) -> Result<(AcreConfig, Repository, RepositoryState)> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    Ok((config, repository, state))
}

pub fn current_workspace<'a>(
    repository: &Repository,
    state: &'a RepositoryState,
) -> Option<&'a WorkspaceRecord> {
    let current_path = repository.current_worktree.as_ref()?.path.as_path();
    state
        .workspaces
        .iter()
        .find(|workspace| canonical_or_absolute(&workspace.path) == canonical_or_absolute(current_path))
}

pub fn require_current_workspace(
    repository: &Repository,
    state: &RepositoryState,
) -> Result<WorkspaceRecord> {
    if let Some(workspace) = current_workspace(repository, state) {
        return Ok(workspace.clone());
    }
    if repository
        .current_worktree
        .as_ref()
        .is_some_and(|worktree| worktree.is_main)
    {
        return Err(AcreError::new(
            "ACRE_PRIMARY_WORKTREE",
            "The repository's primary worktree cannot be returned to Acre.",
            exit::REFUSED,
        ));
    }
    Err(AcreError::new(
        "ACRE_WORKSPACE_NOT_MANAGED",
        "The current worktree is not managed by Acre.",
        exit::NOT_FOUND,
    ))
}
