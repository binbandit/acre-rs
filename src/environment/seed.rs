use std::fs;
use std::path::Path;

use crate::error::{AcreError, Result};
use crate::model::SeedFileSnapshot;
use crate::util::sha256;

pub fn snapshot_seed_files(root: &Path, files: &[String]) -> Result<Vec<SeedFileSnapshot>> {
    let mut snapshots = Vec::new();
    for relative in files {
        if let Some(hash) = seed_path_hash(&root.join(relative))? {
            snapshots.push(SeedFileSnapshot {
                path: relative.clone(),
                hash,
            });
        }
    }
    Ok(snapshots)
}

pub fn changed_seed_files(root: &Path, baseline: &[SeedFileSnapshot]) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for snapshot in baseline {
        if seed_path_hash(&root.join(&snapshot.path))?.as_deref() != Some(&snapshot.hash) {
            changed.push(snapshot.path.clone());
        }
    }
    Ok(changed)
}

fn seed_path_hash(target: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AcreError::io(
                format!("could not inspect {}", target.display()),
                error,
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        let value = fs::read_link(target)
            .map_err(|error| AcreError::io(format!("could not read {}", target.display()), error))?;
        return Ok(Some(sha256(format!("symlink\0{}", value.display()))));
    }
    if metadata.is_file() {
        let mut pieces = Vec::new();
        pieces.extend_from_slice(format!("file\0{}\0", permission_marker(&metadata)).as_bytes());
        pieces.extend_from_slice(
            &fs::read(target)
                .map_err(|error| AcreError::io(format!("could not read {}", target.display()), error))?,
        );
        return Ok(Some(sha256(pieces)));
    }
    if metadata.is_dir() {
        let mut pieces = Vec::new();
        pieces.extend_from_slice(format!("directory\0{}\0", permission_marker(&metadata)).as_bytes());
        let mut entries: Vec<_> = fs::read_dir(target)
            .map_err(|error| AcreError::io(format!("could not read {}", target.display()), error))?
            .filter_map(std::result::Result::ok)
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = seed_path_hash(&entry.path())?.unwrap_or_else(|| "missing".to_owned());
            pieces.extend_from_slice(format!("{name}\0{child}\0").as_bytes());
        }
        return Ok(Some(sha256(pieces)));
    }
    Ok(Some(sha256(format!("other\0{}", permission_marker(&metadata)))))
}

fn permission_marker(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        u32::from(metadata.permissions().readonly())
    }
}
