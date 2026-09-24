//! The environment plan: detected ecosystems, cache roots, seed files, and the generation fingerprint.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value, json};

use crate::environment::definitions::{
    ALL_FINGERPRINT_FILES, EcosystemDefinition, PATCH_DIRECTORY, detect_ecosystems,
};
use crate::environment::roots::is_within_root;
use crate::error::{AcreError, Result, exit};
use crate::git::content::{read_files_at_ref, read_matching_files_at_ref};
use crate::git::repository::Repository;
use crate::model::AcreConfig;
use crate::state::config::load_repo_config;
use crate::util::{sha256, unique_paths};

#[derive(Debug, Clone)]
pub struct EnvironmentPlan {
    pub fingerprint: String,
    pub ecosystem_ids: Vec<String>,
    pub cache_roots: Vec<String>,
    pub required_roots: Vec<String>,
    pub seed_files: Vec<String>,
    pub platform_key: String,
}

pub fn build_environment_plan(
    repository: &Repository,
    reference: &str,
    config: &AcreConfig,
) -> Result<EnvironmentPlan> {
    // Read from the commit, not the worktree: the fingerprint must not depend on uncommitted edits.
    let files = read_matching_files_at_ref(&repository.top_level, reference, ALL_FINGERPRINT_FILES)?;
    let detected = detect_ecosystems(&files);
    let ecosystems: Vec<EcosystemDefinition> = detected
        .iter()
        .map(|found| (found.definition.id, found.definition))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let patches = read_patch_directories(repository, reference, &files)?;
    // .acre.json comes from the primary checkout on disk, not from the target commit.
    let overrides = load_repo_config(repository.primary_path())?
        .environment
        .unwrap_or_default();
    let excluded: BTreeSet<String> = unique_paths(
        config
            .environment
            .excluded_roots
            .iter()
            .chain(overrides.excluded_roots.iter())
            .cloned(),
    )
    .into_iter()
    .collect();

    let cache_roots = unique_paths(
        ecosystems
            .iter()
            .flat_map(|item| item.cache_roots.iter().map(|value| (*value).to_owned()))
            .chain(config.environment.cache_roots.iter().cloned())
            .chain(overrides.cache_roots.iter().cloned()),
    )
    .into_iter()
    .filter(|root| !excluded.contains(root))
    .collect::<Vec<_>>();
    // Only the top-most projects decide readiness: a helper tool's pyproject deep in the tree
    // shouldn't hold a Node app at warm until someone builds a venv for it.
    let top_depth = detected.iter().map(|found| depth(&found.directory)).min();
    let required_roots = unique_paths(
        detected
            .iter()
            .filter(|found| Some(depth(&found.directory)) == top_depth)
            .flat_map(|found| {
                found
                    .definition
                    .required_roots
                    .iter()
                    .map(|root| join_relative(&found.directory, root))
            })
            .chain(config.environment.required_roots.iter().cloned())
            .chain(overrides.required_roots.iter().cloned()),
    )
    .into_iter()
    // A required root that isn't also a cache root would demand something we never manage.
    .filter(|root| cache_roots.iter().any(|cache| is_within_root(root, cache)))
    .collect::<Vec<_>>();
    let seed_files = unique_paths(
        ecosystems
            .iter()
            .flat_map(|item| item.seed_files.iter().map(|value| (*value).to_owned()))
            .chain(config.environment.seed_files.iter().cloned())
            .chain(overrides.seed_files.iter().cloned()),
    )
    .into_iter()
    .filter(|file| !excluded.contains(file))
    .collect::<Vec<_>>();

    let file_hashes: Vec<Value> = files
        .iter()
        .map(|(file, content)| {
            let normalized = normalize_fingerprint_content(file, content);
            json!([file, sha256(normalized)])
        })
        // A tree object names every blob beneath it, so its hash covers each patch's contents.
        .chain(
            patches
                .iter()
                .map(|(directory, tree)| json!([format!("{directory}/"), sha256(tree)])),
        )
        .collect();
    for seed in &seed_files {
        for cache in &cache_roots {
            if format!("/{seed}/").contains(&format!("/{cache}/")) || cache.starts_with(&format!("{seed}/")) {
                return Err(AcreError::new(
                    "ACRE_CONFIG_INVALID",
                    format!(
                        "Seed path {seed} overlaps cache root {cache}. Keep secrets outside cache roots."
                    ),
                    exit::USAGE,
                ));
            }
        }
    }
    // Native modules and build outputs don't survive an OS or Node major change. Node is asked
    // only for Node projects, from the primary checkout, so the answer doesn't vary with the
    // caller's directory. A caller whose PATH has no node still sees "none": a spurious cold
    // start is the price of never presenting another Node's native modules as compatible.
    let node = if ecosystems.iter().any(|item| NODE_ECOSYSTEMS.contains(&item.id)) {
        node_major(repository.primary_path()).unwrap_or_else(|| "none".to_owned())
    } else {
        "unused".to_owned()
    };
    let platform_key = format!(
        "{}:{}:{}:{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        os_major(),
        node
    );
    let ecosystem_ids: Vec<String> = ecosystems.iter().map(|item| item.id.to_owned()).collect();
    let fingerprint_value = json!({
        "platformKey": platform_key,
        "ecosystems": ecosystem_ids,
        "files": file_hashes,
        "cacheRoots": cache_roots,
        "requiredRoots": required_roots,
    });
    // serde_json sorts map keys, so this serialisation is stable across runs.
    let fingerprint = sha256(serde_json::to_vec(&fingerprint_value)?);

    Ok(EnvironmentPlan {
        fingerprint,
        ecosystem_ids,
        cache_roots,
        required_roots,
        seed_files,
        platform_key,
    })
}

fn normalize_fingerprint_content(file: &str, content: &[u8]) -> Vec<u8> {
    if file.rsplit('/').next() != Some("package.json") {
        return content.to_vec();
    }
    // Unparseable JSON is hashed as-is; better a spurious miss than a wrong hit.
    let Ok(parsed) = serde_json::from_slice::<Value>(content) else {
        return content.to_vec();
    };
    let Some(object) = parsed.as_object() else {
        return content.to_vec();
    };
    // Only the keys that change what an install produces; editing scripts.test shouldn't invalidate caches.
    let mut relevant = Map::new();
    for key in [
        "name",
        "version",
        "bin",
        "os",
        "cpu",
        "libc",
        "bundledDependencies",
        "bundleDependencies",
        "packageManager",
        "engines",
        "volta",
        "workspaces",
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
        "peerDependenciesMeta",
        "overrides",
        "resolutions",
        "pnpm",
    ] {
        if let Some(value) = object.get(key) {
            relevant.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(scripts) = object.get("scripts").and_then(Value::as_object) {
        let mut install_scripts = Map::new();
        // Lifecycle scripts run during install, so they do shape node_modules.
        for key in ["preinstall", "install", "postinstall", "prepare"] {
            if let Some(value) = scripts.get(key) {
                install_scripts.insert(key.to_owned(), value.clone());
            }
        }
        if !install_scripts.is_empty() {
            relevant.insert("scripts".to_owned(), Value::Object(install_scripts));
        }
    }
    serde_json::to_vec(&Value::Object(relevant)).unwrap_or_else(|_| content.to_vec())
}

const NODE_ECOSYSTEMS: &[&str] = &["pnpm", "yarn", "npm", "bun", "turbo", "next"];

/// Every package directory's `patches/` tree at `reference`, keyed by that directory.
fn read_patch_directories(
    repository: &Repository,
    reference: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let directories: Vec<String> = files
        .keys()
        .filter_map(|path| match path.rsplit_once('/') {
            Some((directory, "package.json")) => Some(join_relative(directory, PATCH_DIRECTORY)),
            None if path == "package.json" => Some(PATCH_DIRECTORY.to_owned()),
            _ => None,
        })
        .collect();
    let requested: Vec<&str> = directories.iter().map(String::as_str).collect();
    read_files_at_ref(&repository.top_level, reference, &requested)
}

fn depth(directory: &str) -> usize {
    if directory.is_empty() {
        0
    } else {
        directory.split('/').count()
    }
}

fn join_relative(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

/// Kernel release major: on macOS that tracks the OS version, which native modules care about.
/// Read without PATH lookup, so every caller on one machine agrees.
fn os_major() -> String {
    let release = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/uname")
            .arg("-r")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()
    } else {
        None
    };
    release
        .as_deref()
        .and_then(|release| release.trim().split('.').next())
        .filter(|major| !major.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn node_major(directory: &Path) -> Option<String> {
    let output = Command::new("node")
        .arg("--version")
        // Version managers pick node by directory; the primary checkout is the stable choice.
        .current_dir(directory)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_json_script_noise_is_ignored() {
        let a = br#"{"scripts":{"test":"one","postinstall":"x"},"dependencies":{"a":"1"}}"#;
        let b = br#"{"scripts":{"test":"two","postinstall":"x"},"dependencies":{"a":"1"}}"#;
        assert_eq!(
            normalize_fingerprint_content("package.json", a),
            normalize_fingerprint_content("package.json", b)
        );
    }

    #[test]
    fn workspace_identity_and_install_links_affect_fingerprints() {
        for field in ["name", "version", "bin"] {
            let before = serde_json::json!({field: "before"}).to_string();
            let after = serde_json::json!({field: "after"}).to_string();
            assert_ne!(
                normalize_fingerprint_content("packages/a/package.json", before.as_bytes()),
                normalize_fingerprint_content("packages/a/package.json", after.as_bytes()),
                "{field}"
            );
        }
    }
}
