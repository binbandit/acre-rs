use crate::error::Result;
use crate::model::{AcreConfig, ShellSessionState};
use crate::state::paths::shell_state_path;
use crate::util::{now_iso, random_id, read_json, write_json};
use std::path::Path;

pub fn new_shell_session_id() -> String {
    random_id()
}

pub fn read_shell_state(config: &AcreConfig, id: &str) -> Result<Option<ShellSessionState>> {
    read_json(&shell_state_path(config, id))
}

pub fn record_navigation(
    config: &AcreConfig,
    id: &str,
    from: &Path,
    to: &Path,
    pid: Option<u32>,
) -> Result<()> {
    let current = read_shell_state(config, id)?;
    let previous = match current
        .as_ref()
        .and_then(|state| state.current_directory.as_ref())
    {
        Some(directory) if directory != to => Some(directory.clone()),
        _ => Some(from.to_path_buf()),
    };
    let state = ShellSessionState {
        schema_version: 1,
        id: id.to_owned(),
        current_directory: Some(to.to_path_buf()),
        previous_directory: previous,
        pid,
        updated_at: now_iso(),
    };
    write_json(&shell_state_path(config, id), &state)
}
