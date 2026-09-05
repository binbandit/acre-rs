//! Leases record who is using a workspace: a shell session or a machine client holding it open.

use std::path::Path;

use crate::cli::ShellBridge;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::Repository;
use crate::model::{AcreConfig, RepositoryState, WorkspaceLease, WorkspaceRecord};
use crate::state::repository::LockedRepository;
use crate::util::{now_iso, random_id};

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub holder: String,
    pub pid: Option<u32>,
    pub session_id: Option<String>,
}

impl LeaseRequest {
    /// The lease held by the invoking shell session, when shell integration is active.
    pub fn for_shell(shell: &ShellBridge) -> Option<Self> {
        let session_id = shell.session_id.clone().filter(|_| shell.active)?;
        Some(Self {
            // Holder names are for humans in `acre system inspect`; the id is what identifies the lease.
            holder: format!("shell:{session_id}"),
            pid: shell.pid,
            session_id: Some(session_id),
        })
    }
}

/// Records a new lease on `workspace_id`. A shell session holds at most one lease, so its
/// previous one is dropped first.
pub fn acquire(state: &mut RepositoryState, workspace_id: &str, request: &LeaseRequest) -> WorkspaceLease {
    if let Some(session_id) = &request.session_id {
        release_session(state, session_id, None);
    }
    let timestamp = now_iso();
    let lease = WorkspaceLease {
        id: random_id(),
        workspace_id: workspace_id.to_owned(),
        holder: request.holder.clone(),
        pid: request.pid,
        session_id: request.session_id.clone(),
        acquired_at: timestamp.clone(),
        updated_at: timestamp,
    };
    state.leases.push(lease.clone());
    lease
}

pub fn release(state: &mut RepositoryState, lease_id: &str) -> Result<WorkspaceLease> {
    let index = state
        .leases
        .iter()
        .position(|lease| lease.id == lease_id)
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_LEASE_NOT_FOUND",
                "That Acre lease no longer exists.",
                exit::NOT_FOUND,
            )
            .with_details(serde_json::json!({ "leaseId": lease_id }))
        })?;
    Ok(state.leases.remove(index))
}

/// Drops every lease `session_id` holds, or only its lease on `workspace_id` when given.
pub fn release_session(state: &mut RepositoryState, session_id: &str, workspace_id: Option<&str>) {
    state.leases.retain(|lease| {
        lease.session_id.as_deref() != Some(session_id)
            || workspace_id.is_some_and(|workspace| workspace != lease.workspace_id)
    });
}

pub fn lease_workspace_by_path(
    config: &AcreConfig,
    repository: &Repository,
    workspace_path: &Path,
    request: &LeaseRequest,
) -> Result<Option<WorkspaceLease>> {
    let mut locked = LockedRepository::open(config, repository)?;
    let Some(workspace_id) = locked
        .state
        .workspace_at(
            &crate::git::worktrees::find_current_worktree(&repository.worktrees, workspace_path)
                .map(|worktree| worktree.path)
                .unwrap_or_else(|| workspace_path.to_path_buf()),
        )
        .map(|workspace| workspace.id.clone())
    else {
        return Ok(None);
    };
    let lease = acquire(&mut locked.state, &workspace_id, request);
    locked.save(config)?;
    Ok(Some(lease))
}

pub fn release_shell_session_lease(
    config: &AcreConfig,
    repository: &Repository,
    session_id: &str,
) -> Result<()> {
    let mut locked = LockedRepository::open(config, repository)?;
    let before = locked.state.leases.len();
    release_session(&mut locked.state, session_id, None);
    // Skip the write when nothing changed; `acre -` calls this on every hop.
    if locked.state.leases.len() != before {
        locked.save(config)?;
    }
    Ok(())
}

pub fn release_workspace_lease(
    config: &AcreConfig,
    repository: &Repository,
    lease_id: &str,
) -> Result<(WorkspaceLease, RepositoryState, WorkspaceRecord)> {
    let mut locked = LockedRepository::open(config, repository)?;
    let lease = release(&mut locked.state, lease_id)?;
    let workspace = locked
        .state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == lease.workspace_id)
        .cloned()
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_WORKSPACE_NOT_FOUND",
                "The leased workspace no longer exists.",
                exit::NOT_FOUND,
            )
        })?;
    locked.save(config)?;
    Ok((lease, locked.state, workspace))
}
