//! Built-in ecosystem knowledge: which files identify a toolchain, which directories it caches,
//! and which build output it regenerates.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct EcosystemDefinition {
    pub id: &'static str,
    pub cache_roots: &'static [&'static str],
    pub required_roots: &'static [&'static str],
    /// Ignored paths the toolchain rebuilds from the code, such as `.next`: removed on return,
    /// never cloned. A cache root inside one, such as `.next/cache`, is kept.
    pub output_roots: &'static [&'static str],
    pub seed_files: &'static [&'static str],
}

/// Files the operating system's file manager leaves in any directory it opens.
pub const SYSTEM_OUTPUT_ROOTS: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

/// What every JavaScript project shares, whichever package manager installs it.
const JAVASCRIPT: EcosystemDefinition = EcosystemDefinition {
    id: "javascript",
    // Tool caches are keyed by content, so they stay valid across branches and are worth
    // keeping warm beside `node_modules`.
    cache_roots: &[
        ".eslintcache",
        ".stylelintcache",
        ".cache",
        ".parcel-cache",
        ".swc",
        ".rollup.cache",
        ".angular/cache",
        ".nx/cache",
        ".vitepress/cache",
    ],
    required_roots: &[],
    // Bundles, reports, logs, packed tarballs, and the directories frameworks generate from one
    // branch's routes and types.
    output_roots: &[
        "dist",
        "build",
        "coverage",
        ".nyc_output",
        "*.lcov",
        "*.tsbuildinfo",
        "*.log",
        "*.tgz",
        "storybook-static",
        "playwright-report",
        "test-results",
        ".output",
        ".nuxt",
        ".svelte-kit",
        ".astro",
        ".docusaurus",
        ".expo",
        ".vinxi",
        ".react-router",
        ".vercel/output",
        ".vitepress/dist",
        // Nx's project graph and daemon files; its task cache inside is a cache root.
        ".nx",
    ],
    seed_files: &[".env", ".env.local"],
};

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
    output_roots: &[],
    seed_files: &[],
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
    output_roots: &[],
    seed_files: &[],
};
const NPM: EcosystemDefinition = EcosystemDefinition {
    id: "npm",
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    output_roots: &[],
    seed_files: &[],
};
const BUN: EcosystemDefinition = EcosystemDefinition {
    id: "bun",
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    output_roots: &[],
    seed_files: &[],
};
const TURBO: EcosystemDefinition = EcosystemDefinition {
    id: "turbo",
    cache_roots: &[".turbo"],
    required_roots: &[],
    output_roots: &[],
    seed_files: &[],
};
const NEXT: EcosystemDefinition = EcosystemDefinition {
    id: "next",
    cache_roots: &[".next/cache"],
    required_roots: &[],
    output_roots: &[".next", "next-env.d.ts", "out"],
    seed_files: &[],
};
const RUST: EcosystemDefinition = EcosystemDefinition {
    id: "rust",
    cache_roots: &["target"],
    required_roots: &[],
    // rustfmt backups and the raw profiles coverage runs leave beside the crates.
    output_roots: &["*.rs.bk", "*.profraw"],
    seed_files: &[".env", ".env.local"],
};
const PYTHON: EcosystemDefinition = EcosystemDefinition {
    id: "python",
    // `.tox` and `.nox` hold virtual environments, which are only ever reused in place.
    cache_roots: &[
        ".venv",
        "venv",
        ".tox",
        ".nox",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".hypothesis",
        ".pytype",
        ".pyre",
    ],
    required_roots: &[".venv"],
    // Packaging and coverage output.
    output_roots: &[
        "build",
        "dist",
        "*.egg-info",
        ".eggs",
        ".pdm-build",
        "htmlcov",
        ".coverage",
        "coverage.xml",
        ".dmypy.json",
        "*.log",
    ],
    seed_files: &[".env", ".env.local"],
};
const GO: EcosystemDefinition = EcosystemDefinition {
    id: "go",
    // Go's caches live outside the repository, so there is nothing to keep warm inside it.
    cache_roots: &[],
    required_roots: &[],
    // Compiled binaries, test binaries, and coverage profiles.
    output_roots: &["bin", "dist", "*.test", "*.out", "*.coverprofile"],
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
    let declared = package_json["packageManager"]
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
    let package_manager = if files.contains_key("pnpm-lock.yaml") || declared == "pnpm" {
        Some(PNPM)
    } else if files.contains_key("yarn.lock") || declared == "yarn" {
        Some(YARN)
    } else if files.contains_key("bun.lock") || files.contains_key("bun.lockb") || declared == "bun" {
        Some(BUN)
    } else if files.contains_key("package-lock.json")
        || files.contains_key("npm-shrinkwrap.json")
        || files.contains_key("package.json")
    {
        Some(NPM)
    } else {
        None
    };
    result.extend(
        package_manager
            .into_iter()
            .flat_map(|manager| [JAVASCRIPT, manager]),
    );

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
