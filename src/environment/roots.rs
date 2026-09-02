use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::git::status::list_ignored_entries;

/// Where a worktree's ignored data lives: approved cache-root directories, and the
/// Git-ignored paths that fall outside every one of them.
#[derive(Debug)]
pub struct IgnoredLayout {
    pub cache_roots: Vec<String>,
    pub unknown: Vec<String>,
}

impl IgnoredLayout {
    /// True when a directory matching `root` is present at any depth.
    pub fn has_cache_root(&self, root: &str) -> bool {
        self.cache_roots
            .iter()
            .any(|present| is_within_root(present, root))
    }
}

/// Cache roots apply at any depth, so a monorepo package's `node_modules` counts like the
/// top-level one. Git collapses ignored directories to their topmost entry, so a collapsed
/// entry is opened up when it holds nothing but cache roots, and a multi-component root such
/// as `.next/cache` is completed inside its ignored parent.
pub fn inspect_ignored(worktree: &Path, cache_roots: &[String]) -> Result<IgnoredLayout> {
    let mut found: BTreeSet<String> = cache_roots
        .iter()
        .filter(|root| worktree.join(root).exists())
        .cloned()
        .collect();
    let mut unknown = Vec::new();
    for entry in list_ignored_entries(worktree)? {
        let relative = entry.trim_end_matches('/');
        if cache_roots.iter().any(|root| is_within_root(relative, root)) {
            found.insert(relative.to_owned());
            continue;
        }
        if entry.ends_with('/') {
            if let Some(nested) = cache_roots_filling(worktree, relative, cache_roots) {
                found.extend(nested);
                continue;
            }
            found.extend(
                cache_roots
                    .iter()
                    .filter_map(|root| nested_cache_root(relative, root))
                    .filter(|nested| worktree.join(nested).is_dir()),
            );
        }
        unknown.push(entry);
    }
    Ok(IgnoredLayout {
        cache_roots: without_descendants(found),
        unknown,
    })
}

/// Cache-root directories beneath `relative` when the directory holds nothing else, or `None`
/// as soon as a path outside every cache root turns up.
fn cache_roots_filling(worktree: &Path, relative: &str, cache_roots: &[String]) -> Option<Vec<String>> {
    let mut found = Vec::new();
    for child in fs::read_dir(worktree.join(relative)).ok()?.flatten() {
        let child_relative = format!("{relative}/{}", child.file_name().to_string_lossy());
        if cache_roots
            .iter()
            .any(|root| is_within_root(&child_relative, root))
        {
            found.push(child_relative);
            continue;
        }
        if !child.file_type().is_ok_and(|kind| kind.is_dir()) {
            return None;
        }
        found.extend(cache_roots_filling(worktree, &child_relative, cache_roots)?);
    }
    Some(found)
}

/// When an ignored directory is an ancestor of a multi-component cache root, such as
/// `apps/web/.next` for `.next/cache`, complete it to the nested root path.
fn nested_cache_root(entry: &str, root: &str) -> Option<String> {
    if !root.contains('/') {
        return None;
    }
    let root_parts: Vec<&str> = root.split('/').collect();
    let entry_parts: Vec<&str> = entry.split('/').collect();
    (1..root_parts.len()).rev().find_map(|count| {
        entry_parts
            .ends_with(&root_parts[..count])
            .then(|| format!("{entry}/{}", root_parts[count..].join("/")))
    })
}

/// True when the repository-relative `path` is `root` or lies beneath a directory matching
/// `root` at any depth, so `packages/app/node_modules` matches the root `node_modules`.
/// Both sides arrive without `./` prefixes or trailing slashes.
fn is_within_root(path: &str, root: &str) -> bool {
    format!("/{path}/").contains(&format!("/{root}/"))
}

/// Sorted order puts parents first, so anything beneath a kept path is already covered.
fn without_descendants(paths: BTreeSet<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for candidate in paths {
        let covered = result.iter().any(|kept| {
            candidate
                .strip_prefix(kept.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        });
        if !covered {
            result.push(candidate);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::runner::run_git;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn repository(gitignore: &str, files: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-q"]).expect("git init");
        fs::write(temp.path().join(".gitignore"), gitignore).expect("gitignore");
        for file in files {
            let path = temp.path().join(file);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            fs::write(path, "x").expect("file");
        }
        // Stage everything Git does not ignore so the layout reads like a real checkout.
        run_git(temp.path(), &["add", "-A"]).expect("git add");
        temp
    }

    #[test]
    fn classifies_nested_and_collapsed_cache_roots() {
        let repo = repository(
            "node_modules\n*.log\n.next\n",
            &[
                "node_modules/dep/index.js",
                "packages/app/node_modules/dep/index.js",
                "apps/web/src/page.js",
                "apps/web/.next/cache/chunk",
                "apps/web/.next/server/page",
                "stray.log",
            ],
        );
        let layout =
            inspect_ignored(repo.path(), &strings(&["node_modules", ".next/cache"])).expect("layout");
        assert_eq!(
            layout.cache_roots,
            strings(&[
                "apps/web/.next/cache",
                "node_modules",
                "packages/app/node_modules"
            ])
        );
        assert_eq!(layout.unknown, strings(&["apps/web/.next/", "stray.log"]));
        assert!(layout.has_cache_root("node_modules"));
        assert!(layout.has_cache_root(".next/cache"));
        assert!(!layout.has_cache_root(".venv"));
    }

    #[test]
    fn collapsed_directory_with_other_content_stays_unknown() {
        let repo = repository(
            "packages/\n",
            &[
                "packages/app/node_modules/dep/index.js",
                "packages/app/notes.txt.bak",
            ],
        );
        let layout = inspect_ignored(repo.path(), &strings(&["node_modules"])).expect("layout");
        assert!(layout.cache_roots.is_empty(), "{:?}", layout.cache_roots);
        assert_eq!(layout.unknown, strings(&["packages/"]));
    }

    #[test]
    fn root_matching_works_at_any_depth() {
        assert!(is_within_root("node_modules", "node_modules"));
        assert!(is_within_root("packages/app/node_modules", "node_modules"));
        assert!(is_within_root("node_modules/.cache/x", "node_modules"));
        assert!(is_within_root("apps/web/.next/cache", ".next/cache"));
        assert!(!is_within_root("node_modules_backup", "node_modules"));
        assert!(!is_within_root("apps/web/.next", ".next/cache"));
    }

    #[test]
    fn nested_root_requires_a_component_boundary() {
        assert_eq!(
            nested_cache_root("apps/web/.next", ".next/cache").as_deref(),
            Some("apps/web/.next/cache")
        );
        assert_eq!(nested_cache_root("apps/web", ".next/cache"), None);
        assert_eq!(nested_cache_root("node_modules", "node_modules"), None);
    }

    #[test]
    fn descendants_of_a_kept_root_are_dropped() {
        let paths: BTreeSet<String> = strings(&["node_modules", "node_modules/.cache", "node_modules-x"])
            .into_iter()
            .collect();
        assert_eq!(
            without_descendants(paths),
            strings(&["node_modules", "node_modules-x"])
        );
    }
}
