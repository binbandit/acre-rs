use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::environment::inspect::inspect_environment;
use crate::error::{AcreError, Result};
use crate::git::runner::{RunOptions, run_git_with, run_process};
use crate::model::{CloneMode, EnvironmentPlan, EnvironmentSnapshot, TrustLevel};
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

    for cache_root in &plan.cache_roots {
        let source = source_root.join(cache_root);
        let destination = destination_root.join(cache_root);
        if !source.exists() {
            continue;
        }
        remove_path(&destination)?;
        if let Some(parent) = destination.parent() {
            ensure_directory(parent)?;
        }
        let report = clone_tree(&source, &destination)?;
        cloned_files += report.files;
        cloned_bytes += report.bytes;
        clone_mode = match (clone_mode, report.mode) {
            (_, CloneMode::Copy) => CloneMode::Copy,
            (CloneMode::None, CloneMode::Reflink) => CloneMode::Reflink,
            (current, _) => current,
        };
    }

    let seeded_files = seed_files_only(seed_source_root, destination_root, &plan.seed_files, trust)?;
    let mut snapshot = inspect_environment(destination_root, plan);
    snapshot.source = Some(source_root.to_path_buf());
    snapshot.cloned_files = Some(cloned_files);
    snapshot.cloned_bytes = Some(cloned_bytes);
    snapshot.clone_mode = Some(clone_mode);
    Ok(SeedResult {
        snapshot,
        seeded_files,
    })
}

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
        match copy_seed_path(&source, &destination) {
            Ok(()) => seeded.push(relative.clone()),
            Err(_) => {
                let _ = remove_path(&destination);
            }
        }
    }
    Ok(seeded)
}

pub fn clear_cache_roots(root: &Path, cache_roots: &[String]) -> Result<()> {
    for relative in cache_roots {
        remove_path(&root.join(relative))?;
    }
    Ok(())
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
        vec![
            "check-ignore".into(),
            "--quiet".into(),
            "--".into(),
            relative.into(),
        ],
        RunOptions {
            timeout: Some(Duration::from_secs(10)),
            accepted_statuses: &[0, 1, 128],
            ..RunOptions::default()
        },
    )?;
    Ok(result.status == 0)
}

fn copy_seed_path(source: &Path, destination: &Path) -> Result<()> {
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
    let mut report = CloneReport {
        mode: CloneMode::Reflink,
        files: 0,
        bytes: 0,
    };
    clone_recursively(source, destination, &mut report)?;
    Ok(report)
}

fn clone_with_platform_tool(source: &Path, destination: &Path) -> Option<CloneReport> {
    let (program, args) = if cfg!(target_os = "macos") {
        (
            "/bin/cp",
            vec![
                "-cR".to_owned(),
                source.display().to_string(),
                destination.display().to_string(),
            ],
        )
    } else if cfg!(target_os = "linux") {
        (
            "cp",
            vec![
                "-a".to_owned(),
                "--reflink=always".to_owned(),
                source.display().to_string(),
                destination.display().to_string(),
            ],
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

pub fn find_copy_source(candidates: &[PathBuf], excluded: &Path) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| path.as_path() != excluded && path.exists())
        .cloned()
}
