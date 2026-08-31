use std::path::{Path, PathBuf};

use crate::model::{AcreConfig, Repository};
use crate::util::{absolute, home_dir, repository_slug};

pub fn default_acre_root() -> PathBuf {
    home_dir().join(".acre")
}

pub fn acre_root(config: &AcreConfig) -> PathBuf {
    absolute(&config.root)
}

pub fn config_path() -> PathBuf {
    std::env::var_os("ACRE_CONFIG")
        .map(PathBuf::from)
        .map(|path| absolute(&path))
        .unwrap_or_else(|| default_acre_root().join("config.json"))
}

pub fn repository_index_path(config: &AcreConfig) -> PathBuf {
    acre_root(config).join("repositories.json")
}

pub fn repository_root(config: &AcreConfig, repository: &Repository) -> PathBuf {
    acre_root(config)
        .join("repositories")
        .join(repository_slug(&repository.name, &repository.id))
}

pub fn repository_state_path(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("state.json")
}

pub fn repository_lock_path(config: &AcreConfig, repository: &Repository) -> PathBuf {
    repository_root(config, repository).join("lock")
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

pub fn operation_path(config: &AcreConfig, token: &str) -> PathBuf {
    operations_root(config).join(format!("{token}.json"))
}

pub fn shells_root(config: &AcreConfig) -> PathBuf {
    acre_root(config).join("shells")
}

pub fn shell_state_path(config: &AcreConfig, id: &str) -> PathBuf {
    shells_root(config).join(format!("{id}.json"))
}

pub fn belongs_to_root(root: &Path, candidate: &Path) -> bool {
    crate::util::is_inside(root, candidate)
}
