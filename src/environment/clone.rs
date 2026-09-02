//! Reuses prepared caches: clones approved cache roots between worktrees, ideally as reflinks.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::environment::fingerprint::EnvironmentPlan;
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::inspect_ignored;
use crate::environment::seed::seed_files_only;
use crate::error::{AcreError, Result};
use crate::git::runner::{RunOptions, run_process};
use crate::model::{CloneMode, EnvironmentSnapshot, TrustLevel};
use crate::util::{ensure_directory, remove_path};

#[derive(Debug, Clone)]
pub struct SeedResult {
    pub snapshot: EnvironmentSnapshot,
    pub seeded_files: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct CloneReport {
    pub mode: CloneMode,
    pub files: u64,
    pub bytes: u64,
}

pub fn seed_environment(
    source_root: &Path,
    destination_root: &Path,
    plan: &EnvironmentPlan,
    trust: TrustLevel,
    seed_source_root: &Path,
) -> Result<SeedResult> {
    let mut cloned_files = 0;
    let mut cloned_bytes = 0;
    let mut clone_mode = CloneMode::None;

    for cache_root in inspect_ignored(source_root, &plan.cache_roots)?.cache_roots {
        let source = source_root.join(&cache_root);
        let destination = destination_root.join(&cache_root);
        // The destination may hold a stale cache from a previous generation.
        remove_path(&destination)?;
        if let Some(parent) = destination.parent() {
            ensure_directory(parent)?;
        }
        let report = clone_tree(&source, &destination)?;
        cloned_files += report.files;
        cloned_bytes += report.bytes;
        // Copy is sticky: one plain copy means the result is not honestly a reflink.
        clone_mode = match (clone_mode, report.mode) {
            (_, CloneMode::Copy) => CloneMode::Copy,
            (CloneMode::None, CloneMode::Reflink) => CloneMode::Reflink,
            (current, _) => current,
        };
    }

    let seeded_files = seed_files_only(seed_source_root, destination_root, &plan.seed_files, trust)?;
    let mut snapshot = inspect_environment(destination_root, plan)?;
    snapshot.source = Some(source_root.to_path_buf());
    snapshot.cloned_files = Some(cloned_files);
    snapshot.cloned_bytes = Some(cloned_bytes);
    snapshot.clone_mode = Some(clone_mode);
    Ok(SeedResult {
        snapshot,
        seeded_files,
    })
}

pub fn clear_cache_roots(root: &Path, cache_roots: &[String]) -> Result<()> {
    for relative in inspect_ignored(root, cache_roots)?.cache_roots {
        remove_path(&root.join(relative))?;
    }
    Ok(())
}

/// Copies one path of any kind (file, directory, or symlink) preserving its type and permissions.
pub fn copy_path(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| AcreError::io(format!("could not inspect {}", source.display()), error))?;
    if metadata.file_type().is_symlink() {
        return copy_symlink(source, destination);
    }
    if metadata.is_dir() {
        clone_tree(source, destination)?;
        return Ok(());
    }
    fs::copy(source, destination)
        .map_err(|error| AcreError::io(format!("could not copy {}", source.display()), error))?;
    copy_permissions(source, destination);
    Ok(())
}

pub fn clone_tree(source: &Path, destination: &Path) -> Result<CloneReport> {
    if let Some(report) = clone_with_platform_tool(source, destination) {
        return Ok(report);
    }
    // Optimistic: the first file that has to be copied flips it to Copy.
    let mut report = CloneReport {
        mode: CloneMode::Reflink,
        files: 0,
        bytes: 0,
    };
    clone_recursively(source, destination, &mut report)?;
    Ok(report)
}

fn clone_with_platform_tool(source: &Path, destination: &Path) -> Option<CloneReport> {
    let source_text = source.display().to_string();
    let destination_text = destination.display().to_string();
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        // Apple's cp clones on APFS; the absolute path sidesteps a GNU cp earlier on PATH that lacks -c.
        ("/bin/cp", vec!["-cR", &source_text, &destination_text])
    } else if cfg!(target_os = "linux") {
        (
            "cp",
            // always, not auto: a silent fallback to copying would misreport the mode.
            vec!["-a", "--reflink=always", &source_text, &destination_text],
        )
    } else {
        return None;
    };
    match run_process(
        program,
        &args,
        RunOptions {
            timeout: Some(Duration::from_secs(30 * 60)),
            ..RunOptions::default()
        },
    ) {
        Ok(_) => Some(CloneReport {
            mode: CloneMode::Reflink,
            files: 0,
            bytes: 0,
        }),
        Err(_) => {
            // Start the fallback from a clean slate rather than on top of a half-finished clone.
            let _ = remove_path(destination);
            None
        }
    }
}

fn clone_recursively(source: &Path, destination: &Path, report: &mut CloneReport) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| AcreError::io(format!("could not inspect {}", source.display()), error))?;
    if metadata.file_type().is_symlink() {
        return copy_symlink(source, destination);
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| AcreError::io(format!("could not create {}", destination.display()), error))?;
        for entry in fs::read_dir(source)
            .map_err(|error| AcreError::io(format!("could not read {}", source.display()), error))?
        {
            let entry = entry.map_err(|error| AcreError::io("could not read directory entry", error))?;
            clone_recursively(&entry.path(), &destination.join(entry.file_name()), report)?;
        }
        copy_permissions(source, destination);
        return Ok(());
    }
    if metadata.is_file() {
        fs::copy(source, destination)
            .map_err(|error| AcreError::io(format!("could not copy {}", source.display()), error))?;
        report.mode = CloneMode::Copy;
        report.files += 1;
        report.bytes += metadata.len();
        copy_permissions(source, destination);
    }
    Ok(())
}

fn copy_permissions(source: &Path, destination: &Path) {
    if let Ok(metadata) = fs::metadata(source) {
        let _ = fs::set_permissions(destination, metadata.permissions());
    }
}

fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    let target = fs::read_link(source)
        .map_err(|error| AcreError::io(format!("could not read symlink {}", source.display()), error))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, destination).map_err(|error| {
            AcreError::io(
                format!("could not create symlink {}", destination.display()),
                error,
            )
        })?;
    }
    #[cfg(windows)]
    {
        // Windows needs to know whether the link points at a directory to recreate it.
        let followed = fs::metadata(source).ok();
        let result = if followed.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            std::os::windows::fs::symlink_dir(&target, destination)
        } else {
            std::os::windows::fs::symlink_file(&target, destination)
        };
        result.map_err(|error| {
            AcreError::io(
                format!("could not create symlink {}", destination.display()),
                error,
            )
        })?;
    }
    Ok(())
}
