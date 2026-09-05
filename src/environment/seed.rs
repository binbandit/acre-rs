//! Seed files are trusted local configuration such as `.env`: copied only into trusted
//! workspaces, hashed so any edit blocks return, and scrubbed from idle slots.

use std::fs;
use std::path::Path;

use crate::environment::clone::copy_path;
use crate::error::{AcreError, Result, exit};
use crate::git::status::is_ignored_path;
use crate::model::{SeedFileSnapshot, TrustLevel};
use crate::util::{ensure_directory, has_symlink_parent, remove_path, sha256};

pub fn seed_files_only(
    source_root: &Path,
    destination_root: &Path,
    files: &[String],
    trust: TrustLevel,
    seeded: &mut Vec<String>,
) -> Result<()> {
    // Fork PRs get nothing: their code could read whatever we copy in.
    if trust != TrustLevel::Trusted {
        return Ok(());
    }
    for relative in files {
        let source = source_root.join(relative);
        let destination = destination_root.join(relative);
        // Never overwrite what the checkout already has, and never copy a tracked file as a "seed".
        if has_symlink_parent(source_root, Path::new(relative))?
            || has_symlink_parent(destination_root, Path::new(relative))?
            || !source.try_exists()?
            || destination.symlink_metadata().is_ok()
            || !is_ignored_path(source_root, relative)?
            || !is_ignored_path(destination_root, relative)?
        {
            continue;
        }
        if let Some(parent) = destination.parent() {
            ensure_directory(parent)?;
        }
        let temporary = tempfile::tempdir_in(destination.parent().expect("seed path has a worktree parent"))?;
        let copied = temporary.path().join("seed");
        // Failed copies are cleaned up without publishing a partial secret.
        if copy_path(&source, &copied).is_ok() {
            fs::rename(copied, destination)?;
            seeded.push(relative.clone());
        }
    }
    Ok(())
}
pub fn clear_seed_files(root: &Path, files: &[String]) -> Result<()> {
    for relative in files {
        if has_symlink_parent(root, Path::new(relative))? {
            return Err(AcreError::new(
                "ACRE_UNSAFE_PATH",
                format!("Cannot remove seed file through a symlink: {relative}"),
                exit::REFUSED,
            ));
        }
        remove_path(&root.join(relative))?;
    }
    Ok(())
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
        // A deleted seed file counts as changed too; the user may have moved secrets out on purpose.
        if has_symlink_parent(root, Path::new(&snapshot.path))?
            || seed_path_hash(&root.join(&snapshot.path))?.as_deref() != Some(&snapshot.hash)
        {
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
        // Hash the link target, not what it points at; the file behind it isn't ours.
        return Ok(Some(sha256(format!("symlink\0{}", value.display()))));
    }
    if metadata.is_file() {
        let mut pieces = Vec::new();
        // Mode is part of the hash: a chmod on .env is a change worth blocking on.
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
            .collect::<std::io::Result<_>>()?;
        // Directory order is filesystem-dependent; sort so the hash is stable.
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
