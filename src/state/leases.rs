use crate::error::{AcreError, Result, exit};
use crate::model::{RepositoryState, WorkspaceLease};
use crate::util::{now_iso, random_id};

#[derive(Debug, Clone)]
pub struct AcquireLease {
    pub workspace_id: String,
    pub holder: String,
    pub pid: Option<u32>,
    pub session_id: Option<String>,
}

pub fn acquire_lease(state: &mut RepositoryState, input: AcquireLease) -> WorkspaceLease {
    let timestamp = now_iso();
    if let Some(session_id) = &input.session_id {
        if let Some(existing) = state.leases.iter_mut().find(|lease| {
            lease.workspace_id == input.workspace_id
                && lease.holder == input.holder
                && lease.session_id.as_ref() == Some(session_id)
        }) {
            if input.pid.is_some() {
                existing.pid = input.pid;
            }
            existing.updated_at = timestamp;
            return existing.clone();
        }
    }

    let lease = WorkspaceLease {
        id: random_id(),
        workspace_id: input.workspace_id,
        holder: input.holder,
        pid: input.pid,
        session_id: input.session_id,
        acquired_at: timestamp.clone(),
        updated_at: timestamp,
    };
    state.leases.push(lease.clone());
    lease
}

pub fn release_lease(state: &mut RepositoryState, lease_id: &str) -> Result<WorkspaceLease> {
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

pub fn release_session_lease(state: &mut RepositoryState, session_id: &str, workspace_id: Option<&str>) {
    state.leases.retain(|lease| {
        lease.session_id.as_deref() != Some(session_id)
            || workspace_id.is_some_and(|workspace| workspace != lease.workspace_id)
    });
}
