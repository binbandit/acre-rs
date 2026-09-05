//! The index of repositories Acre has touched, so a lease can be released from anywhere.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::git::repository::Repository;
use crate::model::{AcreConfig, KnownRepository};
use crate::state::lock::RepositoryLock;
use crate::state::paths::repository_index_path;
use crate::state::storage::{read_json, write_json};
use crate::util::now_iso;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryIndex {
    schema_version: u32,
    repositories: Vec<KnownRepository>,
}

pub fn remember_repository(config: &AcreConfig, repository: &Repository) -> Result<()> {
    let _lock = RepositoryLock::acquire(&repository_index_path(config).with_extension("lock"))?;
    let mut repositories = load_repository_index(config)?;
    // Replace rather than append, so a moved checkout updates its paths.
    repositories.retain(|record| record.id != repository.id && record.common_dir != repository.common_dir);
    repositories.push(KnownRepository {
        id: repository.id.clone(),
        name: repository.name.clone(),
        common_dir: repository.common_dir.clone(),
        top_level: repository.top_level.clone(),
        remote_url: repository.remote_url.clone(),
        last_seen_at: now_iso(),
    });
    // Most recent first: release searches the index in order.
    repositories.sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));
    write_json(
        &repository_index_path(config),
        &RepositoryIndex {
            schema_version: 1,
            repositories,
        },
    )
}

pub fn load_repository_index(config: &AcreConfig) -> Result<Vec<KnownRepository>> {
    Ok(read_json::<RepositoryIndex>(&repository_index_path(config))?
        .filter(|index| index.schema_version == 1)
        .map(|index| index.repositories)
        .unwrap_or_default())
}
