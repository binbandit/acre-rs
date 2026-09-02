//! Seed files are trusted local configuration such as `.env`: copied only into trusted
//! workspaces, hashed so any edit blocks return, and scrubbed from idle slots.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::environment::clone::copy_path;
use crate::error::{AcreError, Result};
use crate::git::runner::{RunOptions, run_git_with};
use crate::model::{SeedFileSnapshot, TrustLevel};
use crate::util::{ensure_directory, remove_path, sha256};

pub fn seed_files_only(
    source_root: &Path,
    destination_root: &Path,
    files: &[String],
    trust: TrustLevel,
) -> Result<Vec<String>> {
    if trust != TrustLevel::Trusted {
        return Ok(Vec::new());
    }
    let mut seeded = Vec::new();
    for relative in files {
        let source = source_root.join(relative);
        let destination = destination_root.join(relative);
        if !source.exists() || destination.exists() || !is_ignored_seed(source_root, relative)? {
            continue;
        }
        if let Some(parent) = destination.parent() {
            ensure_directory(parent)?;
        }
        match copy_path(&source, &destination) {
            Ok(()) => seeded.push(relative.clone()),
            Err(_) => {
                let _ = remove_path(&destination);
            }
        }
    }
    Ok(seeded)
}
pub fn clear_seed_files(root: &Path, files: &[String]) -> Result<()> {
    for relative in files {
        remove_path(&root.join(relative))?;
    }
    Ok(())
}
fn is_ignored_seed(source_root: &Path, relative: &str) -> Result<bool> {
    let result = run_git_with(
        source_root,
        &["check-ignore", "--quiet", "--", relative],
        RunOptions {
            timeout: Some(Duration::from_secs(10)),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

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
