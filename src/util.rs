//! Small helpers with no better home: time, ids, hashing, paths, filesystem, slugs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AcreError, Result};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn age_millis(value: &str) -> u128 {
    DateTime::parse_from_rfc3339(value)
        .map(|time| {
            let duration = Utc::now().signed_duration_since(time.with_timezone(&Utc));
            duration.num_milliseconds().max(0) as u128
        })
        // An unparseable timestamp counts as brand new, which is the cautious reading.
        .unwrap_or(0)
}

pub fn sha256(value: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_ref());
    hex::encode(hasher.finalize())
}

pub fn short_hash(value: impl AsRef<[u8]>, length: usize) -> String {
    sha256(value).chars().take(length).collect()
}

pub fn random_id() -> String {
    Uuid::new_v4().simple().to_string()
}

pub fn random_short(length: usize) -> String {
    random_id().chars().take(length).collect()
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    path.to_path_buf()
}

pub fn absolute(path: &Path) -> PathBuf {
    let expanded = expand_home(path);
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    }
}

pub fn canonical_or_absolute(path: &Path) -> PathBuf {
    // Paths that don't exist yet still need a stable form for comparisons.
    fs::canonicalize(path).unwrap_or_else(|_| absolute(path))
}

pub fn is_inside(parent: &Path, child: &Path) -> bool {
    let parent = canonical_or_absolute(parent);
    let child = canonical_or_absolute(child);
    child == parent || child.starts_with(&parent)
}

pub fn display_path(path: &Path) -> String {
    let absolute = canonical_or_absolute(path);
    let home = home_dir();
    if absolute == home {
        return "~".to_owned();
    }
    if absolute.starts_with(&home) {
        if let Ok(relative) = absolute.strip_prefix(&home) {
            return format!("~/{}", relative.display());
        }
    }
    absolute.display().to_string()
}

pub fn branch_slug(branch: &str) -> String {
    let mut output = String::with_capacity(branch.len());
    let mut last_dash = false;
    for character in branch.chars() {
        // The Windows-forbidden set, so a slug is valid on every platform state might be synced to.
        let invalid = character.is_control()
            || matches!(character, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*');
        let next = if invalid { '-' } else { character };
        if next == '-' {
            if !last_dash {
                output.push('-');
            }
            last_dash = true;
        } else {
            output.push(next);
            last_dash = false;
        }
    }
    let mut output = output.trim_matches(|c| matches!(c, '.' | ' ' | '-')).to_owned();
    if output.is_empty() {
        output = "workspace".to_owned();
    }
    // Keep directory names short but distinct: the hash of the full branch breaks ties.
    if output.chars().count() > 72 {
        let prefix: String = output.chars().take(62).collect();
        output = format!("{prefix}--{}", short_hash(branch, 8));
    }
    output.trim_end_matches(['.', ' ']).to_owned()
}

pub fn repository_slug(name: &str, id: &str) -> String {
    let safe: String = branch_slug(name).chars().take(48).collect();
    format!(
        "{}-{}",
        if safe.is_empty() { "repository" } else { &safe },
        &id[..id.len().min(8)]
    )
}

pub fn validate_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains('\0') || Path::new(value).is_absolute() {
        return false;
    }
    // No escaping the repository: config paths are joined onto worktrees.
    !Path::new(value).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

pub fn ensure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|error| AcreError::io(format!("could not create {}", path.display()), error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // State may hold copies of .env; keep it private to the user.
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

pub fn remove_path(path: &Path) -> Result<()> {
    // exists() follows symlinks; a dangling link is still ours to remove.
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| AcreError::io(format!("could not inspect {}", path.display()), error))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
            .map_err(|error| AcreError::io(format!("could not remove {}", path.display()), error))
    } else {
        fs::remove_file(path)
            .map_err(|error| AcreError::io(format!("could not remove {}", path.display()), error))
    }
}

pub fn unique_paths(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim_start_matches("./").trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// True when every character of `needle` appears in `haystack` in order, not necessarily adjacent.
pub fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut remaining = needle.chars().peekable();
    for character in haystack.chars() {
        if remaining.peek() == Some(&character) {
            remaining.next();
        }
    }
    remaining.peek().is_none()
}

pub fn shell_quote(value: &Path) -> String {
    let value = value.to_string_lossy();
    #[cfg(windows)]
    {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_slugs_are_safe_directory_names() {
        assert_eq!(branch_slug("feature/refunds"), "feature-refunds");
        assert_eq!(branch_slug("../weird:name?"), "weird-name");
        assert_eq!(branch_slug("///"), "workspace");
    }
}
