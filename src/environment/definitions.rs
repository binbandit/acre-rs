//! Built-in ecosystem knowledge: which files identify a toolchain and which directories it caches.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct EcosystemDefinition {
    pub id: &'static str,
    pub cache_roots: &'static [&'static str],
    pub required_roots: &'static [&'static str],
    pub seed_files: &'static [&'static str],
}

// Manifest names fingerprinted at every depth, including toolchain files shared across ecosystems.
pub const ALL_FINGERPRINT_FILES: &[&str] = &[
    "package.json",
    // Package manager settings such as pnpm's node-linker change the installed layout.
    ".npmrc",
    ".pnpmfile.cjs",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    ".yarnrc",
    ".yarnrc.yml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "bun.lock",
    "bun.lockb",
    "bunfig.toml",
    "turbo.json",
    "next.config.js",
    "next.config.mjs",
    "next.config.ts",
    ".nvmrc",
    ".node-version",
    ".tool-versions",
    "mise.toml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "pyproject.toml",
    ".python-version",
    "uv.lock",
    "poetry.lock",
    "requirements.txt",
    "Pipfile",
    "Pipfile.lock",
    "setup.py",
    "setup.cfg",
    "go.mod",
    "go.sum",
    "go.work",
    "go.work.sum",
];

/// patch-package applies every file in a package's `patches/` directory during install.
pub const PATCH_DIRECTORY: &str = "patches";

/// Whether a changed repository path can change an environment's fingerprint.
pub fn is_fingerprint_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();
    ALL_FINGERPRINT_FILES.contains(&name) || path.split('/').any(|component| component == PATCH_DIRECTORY)
}

const PNPM: EcosystemDefinition = EcosystemDefinition {
    id: "pnpm",
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const YARN: EcosystemDefinition = EcosystemDefinition {
    id: "yarn",
    // Install output beside the caches: Berry's install state and Plug'n'Play loaders.
    cache_roots: &[
        "node_modules",
        ".yarn/cache",
        ".yarn/unplugged",
        ".yarn/install-state.gz",
        ".yarn/build-state.yml",
        ".pnp.cjs",
        ".pnp.loader.mjs",
        ".pnp.data.json",
    ],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const NPM: EcosystemDefinition = EcosystemDefinition {
    id: "npm",
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const BUN: EcosystemDefinition = EcosystemDefinition {
    id: "bun",
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const TURBO: EcosystemDefinition = EcosystemDefinition {
    id: "turbo",
    cache_roots: &[".turbo"],
    required_roots: &[],
    seed_files: &[],
};
const NEXT: EcosystemDefinition = EcosystemDefinition {
    id: "next",
    cache_roots: &[".next/cache"],
    required_roots: &[],
    seed_files: &[],
};
const RUST: EcosystemDefinition = EcosystemDefinition {
    id: "rust",
    cache_roots: &["target"],
    required_roots: &[],
    seed_files: &[".env", ".env.local"],
};
const PYTHON: EcosystemDefinition = EcosystemDefinition {
    id: "python",
    cache_roots: &[
        ".venv",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
    ],
    required_roots: &[".venv"],
    seed_files: &[".env", ".env.local"],
};
const GO: EcosystemDefinition = EcosystemDefinition {
    id: "go",
    cache_roots: &[],
    required_roots: &[],
    seed_files: &[".env", ".env.local"],
};

/// An ecosystem found in one project directory (`""` for the repository root).
#[derive(Debug, Clone)]
pub struct DetectedEcosystem {
    pub directory: String,
    pub definition: EcosystemDefinition,
}

/// Every ecosystem in every project directory, ordered by directory.
pub fn detect_ecosystems(files: &BTreeMap<String, Vec<u8>>) -> Vec<DetectedEcosystem> {
    let mut directories: BTreeMap<&str, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for (path, content) in files {
        let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
        directories
            .entry(directory)
            .or_default()
            .insert(name.to_owned(), content.clone());
    }
    directories
        .into_iter()
        .flat_map(|(directory, files)| {
            detect_directory_ecosystems(&files)
                .into_iter()
                .map(move |definition| DetectedEcosystem {
                    directory: directory.to_owned(),
                    definition,
                })
        })
        .collect()
}

fn detect_directory_ecosystems(files: &BTreeMap<String, Vec<u8>>) -> Vec<EcosystemDefinition> {
    let mut result = Vec::new();
    let package_json = files
        .get("package.json")
        .and_then(|value| serde_json::from_slice::<serde_json::Value>(value).ok())
        .unwrap_or_default();
    let package_manager = package_json["packageManager"]
        .as_str()
        .and_then(|value| value.split('@').next())
        .unwrap_or_default();
    let has_dependency = |name: &str| {
        [
            "dependencies",
            "devDependencies",
            "optionalDependencies",
            "peerDependencies",
        ]
        .iter()
        .any(|section| package_json[section].get(name).is_some())
    };

    // One package manager per project directory: lockfile, then packageManager, then npm.
    if files.contains_key("pnpm-lock.yaml") || package_manager == "pnpm" {
        result.push(PNPM);
    } else if files.contains_key("yarn.lock") || package_manager == "yarn" {
        result.push(YARN);
    } else if files.contains_key("bun.lock") || files.contains_key("bun.lockb") || package_manager == "bun" {
        result.push(BUN);
    } else if files.contains_key("package-lock.json")
        || files.contains_key("npm-shrinkwrap.json")
        || files.contains_key("package.json")
    {
        result.push(NPM);
    }

    // Tooling stacks on top of the package manager, so these are checked independently.
    if files.contains_key("turbo.json") || has_dependency("turbo") {
        result.push(TURBO);
    }
    if ["next.config.js", "next.config.mjs", "next.config.ts"]
        .iter()
        .any(|file| files.contains_key(*file))
        || has_dependency("next")
    {
        result.push(NEXT);
    }
    if files.contains_key("Cargo.toml") || files.contains_key("Cargo.lock") {
        result.push(RUST);
    }
    if [
        "pyproject.toml",
        "uv.lock",
        "poetry.lock",
        "requirements.txt",
        "Pipfile",
        "Pipfile.lock",
        "setup.py",
        "setup.cfg",
    ]
    .iter()
    .any(|file| files.contains_key(*file))
    {
        result.push(PYTHON);
    }
    if files.contains_key("go.mod") || files.contains_key("go.work") {
        result.push(GO);
    }
    result
}
