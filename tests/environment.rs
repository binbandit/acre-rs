//! Environment fingerprints, readiness, and seeding as a user sees them through the CLI.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Fixture, git};
use serde_json::Value;

fn commit(fixture: &Fixture, files: &[(&str, &str)], message: &str) {
    for (path, contents) in files {
        let path = fixture.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    git(&fixture.repo, &["add", "-A"]);
    git(&fixture.repo, &["commit", "-q", "-m", message]);
}

fn open(fixture: &Fixture, branch: &str) -> Value {
    let opened = fixture.json(&["--json", "new", branch, "--stay"]);
    assert_eq!(opened["ok"], true, "{opened}");
    opened
}

fn fingerprint(opened: &Value) -> String {
    opened["environment"]["fingerprint"].as_str().unwrap().to_owned()
}

#[test]
fn install_configuration_starts_a_new_generation() {
    let f = Fixture::new();
    let before = fingerprint(&open(&f, "before"));
    commit(&f, &[(".npmrc", "node-linker=hoisted\n")], "hoist");
    let npmrc = fingerprint(&open(&f, "npmrc"));
    assert_ne!(before, npmrc, ".npmrc changes how node_modules is laid out");

    commit(&f, &[("patches/dep+1.0.0.patch", "one\n")], "patch");
    let patched = fingerprint(&open(&f, "patched"));
    commit(&f, &[("patches/dep+1.0.0.patch", "two\n")], "repatch");
    let repatched = fingerprint(&open(&f, "repatched"));
    assert_ne!(npmrc, patched);
    assert_ne!(
        patched, repatched,
        "patch-package output depends on patch contents"
    );
}

#[test]
fn pipfile_projects_are_python() {
    let f = Fixture::new();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    commit(&f, &[("Pipfile", "[packages]\n")], "pipenv");
    let opened = open(&f, "pipenv");
    assert!(
        opened["environment"]["cacheRoots"]
            .as_array()
            .unwrap()
            .contains(&".venv".into()),
        "{opened}"
    );
}

#[test]
fn a_fingerprint_does_not_depend_on_the_callers_node() {
    let f = Fixture::new();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    commit(&f, &[("Cargo.toml", "[package]\nname = \"x\"\n")], "rust");
    let plain = fingerprint(&open(&f, "plain"));

    // A different node earlier on PATH, as a GUI editor or version manager might present it.
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("node"), "#!/bin/sh\necho v99.0.0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin.join("node"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let output = f
        .acre(&["--json", "new", "other-node", "--stay"])
        .env("PATH", path)
        .output()
        .unwrap();
    let other: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        plain,
        fingerprint(&other),
        "Rust caches don't care which node is installed"
    );
}

#[test]
fn a_nested_helper_project_does_not_hold_back_readiness() {
    let f = Fixture::new();
    commit(
        &f,
        &[("tools/lint/pyproject.toml", "[project]\nname = \"lint\"\n")],
        "helper",
    );
    fs::create_dir(f.repo.join("node_modules")).unwrap();
    fs::write(f.repo.join("node_modules/marker"), "installed").unwrap();
    let opened = open(&f, "ready");
    assert_eq!(opened["environment"]["state"], "ready", "{opened}");
}

#[test]
fn a_top_level_sibling_project_still_counts_toward_readiness() {
    let f = Fixture::new();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    commit(
        &f,
        &[
            ("web/package.json", "{\"name\":\"web\"}"),
            ("api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ],
        "polyglot",
    );
    fs::create_dir_all(f.repo.join("web/node_modules")).unwrap();
    fs::write(f.repo.join("web/node_modules/marker"), "installed").unwrap();
    let opened = open(&f, "polyglot");
    assert_eq!(
        opened["environment"]["state"], "warm",
        "api/.venv is still missing: {opened}"
    );
}

fn done(f: &Fixture, branch: &str) -> Value {
    f.json(&["--json", "done", branch])
}

#[test]
fn python_bytecode_does_not_block_done() {
    let f = Fixture::new();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    commit(
        &f,
        &[
            (".gitignore", "__pycache__/\n.venv\n"),
            ("pyproject.toml", "[project]\nname = \"app\"\n"),
            ("app/m.py", "x = 1\n"),
        ],
        "python",
    );
    let path = PathBuf::from(open(&f, "py")["path"].as_str().unwrap());
    fs::create_dir_all(path.join("app/__pycache__")).unwrap();
    fs::write(path.join("app/__pycache__/m.cpython-314.pyc"), "bytecode").unwrap();
    let returned = done(&f, "py");
    assert_eq!(returned["ok"], true, "{returned}");
}

#[test]
fn yarn_install_state_does_not_block_done() {
    let f = Fixture::new();
    commit(
        &f,
        &[
            (".gitignore", ".yarn/*\n!.yarn/releases\nnode_modules\n.pnp.*\n"),
            ("yarn.lock", "# yarn lockfile v1\n"),
        ],
        "yarn",
    );
    let path = PathBuf::from(open(&f, "yarn")["path"].as_str().unwrap());
    fs::create_dir_all(path.join(".yarn")).unwrap();
    fs::write(path.join(".yarn/install-state.gz"), "state").unwrap();
    fs::write(path.join(".pnp.cjs"), "module.exports = {}").unwrap();
    let returned = done(&f, "yarn");
    assert_eq!(returned["ok"], true, "{returned}");
}

#[cfg(unix)]
#[test]
fn a_relative_seed_symlink_still_resolves_in_the_workspace() {
    let f = Fixture::new();
    let shared = f.temp.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::write(shared.join("app.env"), "TOKEN=1\n").unwrap();
    commit(&f, &[(".gitignore", "node_modules\n.env\n")], "ignore env");
    std::os::unix::fs::symlink(std::path::Path::new("../shared/app.env"), f.repo.join(".env")).unwrap();
    let path = PathBuf::from(open(&f, "linked")["path"].as_str().unwrap());
    assert_eq!(fs::read_to_string(path.join(".env")).unwrap(), "TOKEN=1\n");
    // Still a link: edits to the shared file reach every workspace.
    assert!(
        fs::symlink_metadata(path.join(".env"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn a_relative_seed_symlink_into_the_checkout_stays_relative() {
    let f = Fixture::new();
    commit(
        &f,
        &[
            (".gitignore", "node_modules\n.env\n"),
            ("config/dev.env", "TOKEN=tracked\n"),
        ],
        "tracked env",
    );
    std::os::unix::fs::symlink(std::path::Path::new("config/dev.env"), f.repo.join(".env")).unwrap();
    let path = PathBuf::from(open(&f, "inside")["path"].as_str().unwrap());
    // The workspace's own tracked copy, not the primary's.
    assert_eq!(
        fs::read_link(path.join(".env")).unwrap(),
        std::path::Path::new("config/dev.env")
    );
}
