//! Atomic JSON files: every persisted record is written to a temporary file and renamed into place.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AcreError, Result};
use crate::util::ensure_directory;
use tempfile::NamedTempFile;

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AcreError::json(format!("could not parse {}", path.display()), error)),
        // Missing is an ordinary answer; every caller has a default for it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AcreError::io(format!("could not read {}", path.display()), error)),
    }
}
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AcreError::json(format!("could not encode {}", path.display()), error))?;
    // Trailing newline so the file diffs and cats cleanly.
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_directory(parent)?;
    // Same-filesystem replacement, owner-only permissions, and cleanup on every error path.
    let mut file = NamedTempFile::new_in(parent).map_err(|error| {
        AcreError::io(
            format!("could not create a temporary file in {}", parent.display()),
            error,
        )
    })?;
    file.write_all(bytes)
        .map_err(|error| AcreError::io(format!("could not write {}", path.display()), error))?;
    // Flush the data before the rename, or a crash could leave a complete-looking empty file.
    file.as_file()
        .sync_all()
        .map_err(|error| AcreError::io(format!("could not sync {}", path.display()), error))?;
    file.persist(path)
        .map_err(|error| AcreError::io(format!("could not replace {}", path.display()), error.error))?;

    // Sync the directory too, so the rename itself survives a power cut.
    #[cfg(unix)]
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
