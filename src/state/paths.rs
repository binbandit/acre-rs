//! Where every persisted file lives under the Acre root.

use std::path::PathBuf;

use crate::error::{AcreError, Result, exit};
use crate::git::repository::Repository;
use crate::model::{AcreConfig, RepositoryState};
use crate::state::storage::read_json;
use crate::util::{absolute, expand_home, home_dir};

pub fn default_acre_root() -> PathBuf {
    home_dir().join(".acre")
}

pub fn acre_root(config: &AcreConfig) -> PathBuf {
    let root = expand_home(&config.root);
    if root.is_absolute() {
        root
    } else {
        config_path().parent().expect("config has a parent").join(root)
    }
}

pub fn config_path() -> PathBuf {
    // ACRE_CONFIG exists for tests and scripts that need a fully isolated root.
    std::env::var_os("ACRE_CONFIG")
        .map(PathBuf::from)
        .map(|path| absolute(&path))
        .unwrap_or_else(|| default_acre_root().join("config.json"))
}

pub fn repository_index_path(config: &AcreConfig) -> PathBuf {
    acre_root(config).join("repositories.json")
}

pub fn repository_root(config: &AcreConfig, repository: &Repository) -> PathBuf {
    let repositories = acre_root(config).join("repositories");
    let stable = repositories.join(&repository.id);
    if stable.exists() {
        return stable;
    }
    // Preserve existing paths from both name-keyed and remote-keyed versions.
    let mut existing = std::fs::read_dir(&repositories)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    existing.sort();
    for path in existing {
        if read_json::<RepositoryState>(&path.join("state.json"))
            .ok()
            .flatten()
            .is_some_and(|state| state.repository_common_dir == repository.common_dir)
        {
            return path;
        }
        if repository.worktrees.iter().any(|worktree| {
            crate::util::is_inside(&path.join("workspaces"), &worktree.path)
                || crate::util::is_inside(&path.join("slots"), &worktree.path)
        }) {
            // Recovery must still find older directories whose state was lost or corrupted.
            return path;
        }
    }
    stable
}

pub fn repository_state_path(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("state.json")
}

pub fn repository_lock_path(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("state.lock")
}

pub fn active_root(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("workspaces")
}

pub fn slots_root(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("slots")
}

pub fn operations_root(config: &AcreConfig) -> PathBuf {
    acre_root(config).join("operations")
}

pub fn operation_path(config: &AcreConfig, token: &str) -> Result<PathBuf> {
    validate_state_id(token)?;
    Ok(operations_root(config).join(format!("{token}.json")))
}

pub fn shells_root(config: &AcreConfig) -> PathBuf {
    acre_root(config).join("shells")
}

pub fn shell_state_path(config: &AcreConfig, id: &str) -> Result<PathBuf> {
    validate_state_id(id)?;
    Ok(shells_root(config).join(format!("{id}.json")))
}

pub fn validate_state_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AcreError::new(
            "ACRE_INVALID_STATE_ID",
            "Invalid Acre session or operation identifier.",
            exit::USAGE,
        ));
    }
    Ok(())
}
