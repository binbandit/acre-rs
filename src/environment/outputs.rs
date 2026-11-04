//! Build output: ignored paths a tool regenerates from the code, removed when a workspace returns.

use std::fs;
use std::path::Path;

use crate::environment::roots::inspect_ignored;
use crate::error::{AcreError, Result};
use crate::util::remove_path;

/// Removes the build output in `worktree` and returns the paths it removed. A cache root inside
/// one, such as `.next/cache` inside `.next`, stays for the next branch.
pub fn clear_build_outputs(
    worktree: &Path,
    cache_roots: &[String],
    output_roots: &[String],
) -> Result<Vec<String>> {
    let mut layout = inspect_ignored(worktree, cache_roots)?;
    let outputs = layout.take_outputs(worktree, output_roots)?;
    for output in &outputs {
        clear_beside_caches(worktree, output, &layout.cache_roots)?;
    }
    Ok(outputs)
}

/// Removes `relative`, or, when cache roots lie inside it, everything in it except them.
fn clear_beside_caches(worktree: &Path, relative: &str, caches: &[String]) -> Result<()> {
    let holds_cache = caches.iter().any(|cache| {
        cache
            .strip_prefix(relative)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    if !holds_cache {
        return remove_path(&worktree.join(relative));
    }
    if caches.iter().any(|cache| cache == relative) {
        return Ok(());
    }
    let directory = worktree.join(relative);
    let entries = fs::read_dir(&directory)
        .map_err(|error| AcreError::io(format!("could not read {}", directory.display()), error))?;
    for entry in entries {
        let entry = entry.map_err(|error| AcreError::io("could not read directory entry", error))?;
        let child = format!("{relative}/{}", entry.file_name().to_string_lossy());
        clear_beside_caches(worktree, &child, caches)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::runner::run_git;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn clears_build_output_but_keeps_caches_and_unknown_files() {
        let repo = tempfile::tempdir().expect("temp dir");
        run_git(repo.path(), &["init", "-q"]).expect("git init");
        fs::write(
            repo.path().join(".gitignore"),
            ".next/\nnext-env.d.ts\n*.tsbuildinfo\nnotes.txt\n",
        )
        .expect("gitignore");
        for file in [
            "apps/web/.next/cache/webpack/chunk",
            "apps/web/.next/server/page.js",
            "apps/web/.next/BUILD_ID",
            "apps/web/next-env.d.ts",
            "packages/ui/tsconfig.tsbuildinfo",
            "notes.txt",
            "apps/web/page.tsx",
            "packages/ui/index.ts",
        ] {
            let path = repo.path().join(file);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
            fs::write(path, "x").expect("file");
        }
        // Stage the source so the layout reads like a real checkout.
        run_git(repo.path(), &["add", "-A"]).expect("git add");

        let cleared = clear_build_outputs(
            repo.path(),
            &strings(&[".next/cache"]),
            &strings(&[".next", "next-env.d.ts", "*.tsbuildinfo"]),
        )
        .expect("clear");

        assert_eq!(
            cleared,
            strings(&[
                "apps/web/.next",
                "apps/web/next-env.d.ts",
                "packages/ui/tsconfig.tsbuildinfo"
            ])
        );
        let remaining = |file: &str| repo.path().join(file).exists();
        assert!(remaining("apps/web/.next/cache/webpack/chunk"));
        assert!(!remaining("apps/web/.next/server"));
        assert!(!remaining("apps/web/.next/BUILD_ID"));
        assert!(!remaining("apps/web/next-env.d.ts"));
        assert!(!remaining("packages/ui/tsconfig.tsbuildinfo"));
        assert!(
            remaining("notes.txt"),
            "unknown ignored data is never build output"
        );
    }
}
