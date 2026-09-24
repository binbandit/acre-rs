//! Reuses prepared caches: clones approved cache roots between worktrees, ideally as reflinks.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::environment::fingerprint::EnvironmentPlan;
use crate::environment::inspect::inspect_environment;
use crate::environment::roots::{inspect_ignored, overlaps_seed};
use crate::error::{AcreError, Result};
use crate::git::runner::{RunOptions, run_process};
use crate::git::status::is_ignored_path;
use crate::model::{CloneMode, EnvironmentSnapshot};
use crate::util::{ensure_directory, has_symlink_parent, remove_path};

#[derive(Debug, Clone, Copy)]
pub struct CloneReport {
    pub mode: CloneMode,
    pub files: u64,
    pub bytes: u64,
}

/// A cache copy kept outside the destination worktree until it can be published under its lock.
pub struct PreparedEnvironment {
    temporary: tempfile::TempDir,
    source: PathBuf,
    caches: Vec<(String, CloneReport)>,
}

pub fn prepare_environment(
    source_root: &Path,
    destination_root: &Path,
    plan: &EnvironmentPlan,
) -> Result<PreparedEnvironment> {
    let parent = destination_root.parent().expect("worktree has a parent");
    let mut prepared = PreparedEnvironment {
        temporary: tempfile::tempdir_in(parent)?,
        source: source_root.to_path_buf(),
        caches: Vec::new(),
    };
    for cache_root in inspect_ignored(source_root, &plan.cache_roots)?.cache_roots {
        if overlaps_seed(&cache_root, &plan.seed_files) {
            continue;
        }
        let source = source_root.join(&cache_root);
        // Virtual environments embed their installation path and cannot be relocated.
        if source.join("pyvenv.cfg").symlink_metadata().is_ok()
            || destination_root.join(&cache_root).symlink_metadata().is_ok()
            || has_symlink_parent(destination_root, Path::new(&cache_root))?
            || !is_ignored_path(destination_root, &ignore_query(&cache_root, &source))?
        {
            continue;
        }
        let cloned = prepared.temporary.path().join(prepared.caches.len().to_string());
        let report = clone_tree(&source, &cloned)?;
        prepared.caches.push((cache_root, report));
    }
    Ok(prepared)
}

impl PreparedEnvironment {
    pub fn publish(self, destination_root: &Path, plan: &EnvironmentPlan) -> Result<EnvironmentSnapshot> {
        let mut cloned_files = 0;
        let mut cloned_bytes = 0;
        let mut clone_mode = CloneMode::None;

        for (index, (cache_root, report)) in self.caches.iter().enumerate() {
            let destination = destination_root.join(cache_root);
            let cloned = self.temporary.path().join(index.to_string());
            // Preparation may have run without the repository lock. An installation or tracked
            // path that appeared since then belongs to its creator and must not be replaced.
            if destination.symlink_metadata().is_ok()
                || has_symlink_parent(destination_root, Path::new(cache_root))?
                || !is_ignored_path(destination_root, &ignore_query(cache_root, &cloned))?
            {
                continue;
            }
            let parent = destination.parent().expect("cache path has a worktree parent");
            ensure_directory(parent)?;
            fs::rename(&cloned, &destination)?;
            cloned_files += report.files;
            cloned_bytes += report.bytes;
            // Copy is sticky: one plain copy means the result is not honestly a reflink.
            clone_mode = match (clone_mode, report.mode) {
                (_, CloneMode::Copy) => CloneMode::Copy,
                (CloneMode::None, CloneMode::Reflink) => CloneMode::Reflink,
                (current, _) => current,
            };
        }

        let mut snapshot = inspect_environment(destination_root, plan)?;
        snapshot.source = Some(self.source.clone());
        snapshot.cloned_files = Some(cloned_files);
        snapshot.cloned_bytes = Some(cloned_bytes);
        snapshot.clone_mode = Some(clone_mode);
        Ok(snapshot)
    }
}

/// A trailing slash lets directory-only ignore rules such as `node_modules/` match; a file
/// cache root such as `.pnp.cjs` is asked about as a file.
fn ignore_query(cache_root: &str, example: &Path) -> String {
    if fs::symlink_metadata(example).is_ok_and(|metadata| metadata.is_dir()) {
        format!("{cache_root}/")
    } else {
        cache_root.to_owned()
    }
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
    if !metadata.is_file() {
        return Err(AcreError::new(
            "ACRE_UNSUPPORTED_FILE",
            format!("Cannot copy special file {}", source.display()),
            crate::error::exit::REFUSED,
        ));
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
    // A plain copy, whatever the filesystem does underneath: only the platform tool proves a clone.
    let mut report = CloneReport {
        mode: CloneMode::Copy,
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
        // Apple's cp -c silently falls back to a full copy when it can't clone, which would be
        // reported as a reflink. Only use it where cloning is certain; otherwise copy and count.
        if !apfs_clone_possible(source, destination) {
            return None;
        }
        // The absolute path sidesteps a GNU cp earlier on PATH that lacks -c.
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
            // A node_modules clone on a slow disk can genuinely take minutes.
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

/// clonefile(2) works only within one APFS volume: same device, and that device is APFS.
fn apfs_clone_possible(source: &Path, destination: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(parent) = destination.parent() else {
            return false;
        };
        let same_volume = fs::symlink_metadata(source)
            .ok()
            .zip(fs::metadata(parent).ok())
            .is_some_and(|(source, parent)| source.dev() == parent.dev());
        // `df -T apfs` lists the path only when its filesystem is APFS, and exits 1 otherwise.
        same_volume
            && run_process(
                "/bin/df",
                &["-T", "apfs", &parent.display().to_string()],
                RunOptions::default(),
            )
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination);
        false
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
    create_symlink(&target, destination, source)
}

/// Creates `destination` pointing at `target`. `source` is an existing link to the same thing,
/// which Windows consults to decide between a file and a directory link.
pub fn create_symlink(target: &Path, destination: &Path, source: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let _ = source;
        std::os::unix::fs::symlink(target, destination).map_err(|error| {
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
            std::os::windows::fs::symlink_dir(target, destination)
        } else {
            std::os::windows::fs::symlink_file(target, destination)
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
