//! Pending two-phase operations, such as a `done` that resumes after the shell has moved.

use crate::error::Result;
use crate::model::{AcreConfig, PendingDoneOperation};
use crate::state::paths::operation_path;
use crate::state::storage::{read_json, write_json};
use crate::util::remove_path;

pub fn save_pending_done(config: &AcreConfig, operation: &PendingDoneOperation) -> Result<()> {
    write_json(&operation_path(config, &operation.token), operation)
}

pub fn read_pending_done(config: &AcreConfig, token: &str) -> Result<Option<PendingDoneOperation>> {
    Ok(read_json::<PendingDoneOperation>(&operation_path(config, token))?
        .filter(|operation| operation.kind == "done"))
}

pub fn remove_pending_operation(config: &AcreConfig, token: &str) -> Result<()> {
    remove_path(&operation_path(config, token))
}
