//! Per-shell-session navigation history, which powers `acre -`.

use crate::error::Result;
use crate::model::{AcreConfig, ShellSessionState};
use crate::state::paths::shell_state_path;
use crate::state::storage::{read_json, write_json};
use crate::util::{now_iso, random_id};
use std::path::Path;

pub fn new_shell_session_id() -> String {
    random_id()
}

pub fn read_shell_state(config: &AcreConfig, id: &str) -> Result<Option<ShellSessionState>> {
    read_json(&shell_state_path(config, id)?)
}

pub fn record_navigation(
    config: &AcreConfig,
    id: &str,
    from: &Path,
    to: &Path,
    pid: Option<u32>,
) -> Result<()> {
    let previous = if from == to {
        read_shell_state(config, id)?.and_then(|state| state.previous_directory)
    } else {
        Some(from.to_path_buf())
    };
    let state = ShellSessionState {
        schema_version: 1,
        id: id.to_owned(),
        current_directory: Some(to.to_path_buf()),
        previous_directory: previous,
        pid,
        updated_at: now_iso(),
    };
    write_json(&shell_state_path(config, id)?, &state)
}
