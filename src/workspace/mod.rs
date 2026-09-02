//! The workspace lifecycle: resolve a target, activate it in a warm slot, assess it, return it.

use crate::error::{AcreError, exit};

pub mod activate;
pub mod assess;
pub mod lease;
pub mod maintain;
pub mod pool;
pub mod process;
pub mod release;
pub mod resolve;

pub(crate) fn missing_workspace(workspace_id: &str) -> AcreError {
    AcreError::new(
        "ACRE_WORKSPACE_NOT_FOUND",
        "That Acre workspace no longer exists.",
        exit::NOT_FOUND,
    )
    .with_details(serde_json::json!({ "workspaceId": workspace_id }))
}
