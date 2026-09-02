//! Atomic JSON files: every persisted record is written to a temporary file and renamed into place.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AcreError, Result};
use crate::util::{ensure_directory, random_short};

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AcreError::json(format!("could not parse {}", path.display()), error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AcreError::io(format!("could not read {}", path.display()), error)),
    }
}
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AcreError::json(format!("could not encode {}", path.display()), error))?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_directory(parent)?;
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or("state");
    // Same directory as the target, so the final rename stays on one filesystem and is atomic.
    let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), random_short(8)));
    let mut options = OpenOptions::new();
    // create_new refuses to clobber: a colliding name means another writer, not a stale file.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| AcreError::io(format!("could not create {}", temporary.display()), error))?;
    file.write_all(bytes)
        .map_err(|error| AcreError::io(format!("could not write {}", temporary.display()), error))?;
    // Flush the data before the rename, or a crash could leave a complete-looking empty file.
    file.sync_all()
        .map_err(|error| AcreError::io(format!("could not sync {}", temporary.display()), error))?;
    drop(file);

    #[cfg(windows)]
    if path.exists() {
        // Windows won't rename over an existing file.
        let _ = fs::remove_file(path);
    }

    fs::rename(&temporary, path)
        .map_err(|error| AcreError::io(format!("could not replace {}", path.display()), error))?;

    // Sync the directory too, so the rename itself survives a power cut.
    #[cfg(unix)]
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
