//! The environment plan: detected ecosystems, cache roots, seed files, and the generation fingerprint.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::{Map, Value, json};

use crate::environment::definitions::{ALL_FINGERPRINT_FILES, detect_ecosystems};
use crate::error::Result;
use crate::git::content::read_files_at_ref;
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
    let files = read_files_at_ref(&repository.top_level, reference, ALL_FINGERPRINT_FILES)?;
    let ecosystems = detect_ecosystems(&files);
    // .acre.json comes from the primary checkout on disk, not from the target commit.
    let overrides = load_repo_config(&repository.top_level)?
        .environment
        .unwrap_or_default();
    let excluded: BTreeSet<String> = config
        .environment
        .excluded_roots
        .iter()
        .chain(overrides.excluded_roots.iter())
        .cloned()
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
    let required_roots = unique_paths(
        ecosystems
            .iter()
            .flat_map(|item| item.required_roots.iter().map(|value| (*value).to_owned()))
            .chain(config.environment.required_roots.iter().cloned())
            .chain(overrides.required_roots.iter().cloned()),
    )
    .into_iter()
    // A required root that isn't also a cache root would demand something we never manage.
    .filter(|root| cache_roots.contains(root))
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
        .collect();
    // Native modules and build outputs don't survive an OS or Node major change.
    let platform_key = format!(
        "{}:{}:{}:{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        os_major(),
        node_major().unwrap_or_else(|| "none".to_owned())
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
    if file != "package.json" {
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
            relevant.insert(key.to_owned(), canonical(value));
        }
    }
    if let Some(scripts) = object.get("scripts").and_then(Value::as_object) {
        let mut install_scripts = Map::new();
        // Lifecycle scripts run during install, so they do shape node_modules.
        for key in ["preinstall", "install", "postinstall", "prepare"] {
            if let Some(value) = scripts.get(key) {
                install_scripts.insert(key.to_owned(), canonical(value));
            }
        }
        if !install_scripts.is_empty() {
            relevant.insert("scripts".to_owned(), Value::Object(install_scripts));
        }
    }
    serde_json::to_vec(&Value::Object(relevant)).unwrap_or_else(|_| content.to_vec())
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        // Key order is noise; sort so equivalent manifests hash the same.
        Value::Object(object) => {
            let sorted: BTreeMap<String, Value> = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        _ => value.clone(),
    }
}

fn os_major() -> String {
    Command::new("uname")
        // Kernel release major: on macOS that tracks the OS version, which native modules care about.
        .arg("-r")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .split('.')
                .next()
                .unwrap_or("unknown")
                .to_owned()
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn node_major() -> Option<String> {
    let output = Command::new("node").arg("--version").output().ok()?;
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
}
