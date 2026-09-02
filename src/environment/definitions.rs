//! Built-in ecosystem knowledge: which files identify a toolchain and which directories it caches.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct EcosystemDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub fingerprint_files: &'static [&'static str],
    pub cache_roots: &'static [&'static str],
    pub required_roots: &'static [&'static str],
    pub seed_files: &'static [&'static str],
}

// Every file any ecosystem looks at, read in one git call; each definition then picks its own.
pub const ALL_FINGERPRINT_FILES: &[&str] = &[
    "package.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    ".yarnrc.yml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "bun.lock",
    "bun.lockb",
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
    "go.mod",
    "go.sum",
];

const PNPM: EcosystemDefinition = EcosystemDefinition {
    id: "pnpm",
    label: "pnpm",
    fingerprint_files: &[
        "package.json",
        "pnpm-lock.yaml",
        ".nvmrc",
        ".node-version",
        ".tool-versions",
        "mise.toml",
    ],
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const YARN: EcosystemDefinition = EcosystemDefinition {
    id: "yarn",
    label: "Yarn",
    fingerprint_files: &[
        "package.json",
        "yarn.lock",
        ".nvmrc",
        ".node-version",
        ".yarnrc.yml",
    ],
    cache_roots: &["node_modules", ".yarn/cache", ".yarn/unplugged"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const NPM: EcosystemDefinition = EcosystemDefinition {
    id: "npm",
    label: "npm",
    fingerprint_files: &[
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        ".nvmrc",
        ".node-version",
    ],
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const BUN: EcosystemDefinition = EcosystemDefinition {
    id: "bun",
    label: "Bun",
    fingerprint_files: &["package.json", "bun.lock", "bun.lockb"],
    cache_roots: &["node_modules"],
    required_roots: &["node_modules"],
    seed_files: &[".env", ".env.local"],
};
const TURBO: EcosystemDefinition = EcosystemDefinition {
    id: "turbo",
    label: "Turborepo",
    fingerprint_files: &["turbo.json", "package.json"],
    cache_roots: &[".turbo"],
    required_roots: &[],
    seed_files: &[],
};
const NEXT: EcosystemDefinition = EcosystemDefinition {
    id: "next",
    label: "Next.js",
    fingerprint_files: &[
        "package.json",
        "next.config.js",
        "next.config.mjs",
        "next.config.ts",
    ],
    cache_roots: &[".next/cache"],
    required_roots: &[],
    seed_files: &[],
};
const RUST: EcosystemDefinition = EcosystemDefinition {
    id: "rust",
    label: "Rust",
    fingerprint_files: &[
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain",
        "rust-toolchain.toml",
    ],
    cache_roots: &["target"],
    required_roots: &[],
    seed_files: &[".env", ".env.local"],
};
const PYTHON: EcosystemDefinition = EcosystemDefinition {
    id: "python",
    label: "Python",
    fingerprint_files: &[
        "pyproject.toml",
        "uv.lock",
        "poetry.lock",
        "requirements.txt",
        ".python-version",
    ],
    cache_roots: &[".venv", ".pytest_cache", ".mypy_cache", ".ruff_cache"],
    required_roots: &[".venv"],
    seed_files: &[".env", ".env.local"],
};
const GO: EcosystemDefinition = EcosystemDefinition {
    id: "go",
    label: "Go",
    fingerprint_files: &["go.mod", "go.sum"],
    cache_roots: &[],
    required_roots: &[],
    seed_files: &[".env", ".env.local"],
};

pub fn detect_ecosystems(files: &BTreeMap<String, Vec<u8>>) -> Vec<EcosystemDefinition> {
    let mut result = Vec::new();
    // Substring checks on the raw text: cheap, and a false positive only adds a harmless cache root.
    let package_json = files
        .get("package.json")
        .map(|value| String::from_utf8_lossy(value))
        .unwrap_or_default();

    // One package manager per repo: lockfile first, then the packageManager field, npm as the fallback.
    if files.contains_key("pnpm-lock.yaml") || package_json.contains("\"packageManager\": \"pnpm") {
        result.push(PNPM);
    } else if files.contains_key("yarn.lock") || package_json.contains("\"packageManager\": \"yarn") {
        result.push(YARN);
    } else if files.contains_key("bun.lock")
        || files.contains_key("bun.lockb")
        || package_json.contains("\"packageManager\": \"bun")
    {
        result.push(BUN);
    } else if files.contains_key("package-lock.json")
        || files.contains_key("npm-shrinkwrap.json")
        || files.contains_key("package.json")
    {
        result.push(NPM);
    }

    // Tooling stacks on top of the package manager, so these are checked independently.
    if files.contains_key("turbo.json") || package_json.contains("\"turbo\"") {
        result.push(TURBO);
    }
    if ["next.config.js", "next.config.mjs", "next.config.ts"]
        .iter()
        .any(|file| files.contains_key(*file))
        || package_json.contains("\"next\"")
    {
        result.push(NEXT);
    }
    if files.contains_key("Cargo.toml") || files.contains_key("Cargo.lock") {
        result.push(RUST);
    }
    if files.contains_key("pyproject.toml")
        || files.contains_key("uv.lock")
        || files.contains_key("poetry.lock")
        || files.contains_key("requirements.txt")
    {
        result.push(PYTHON);
    }
    if files.contains_key("go.mod") {
        result.push(GO);
    }
    result
}
