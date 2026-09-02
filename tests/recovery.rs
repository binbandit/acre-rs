//! End-to-end coverage for state recovery: repair counts, finishing recovered workspaces,
//! and nested cache roots. Each test drives the real binary against a throwaway repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use acre::git::runner::run_git;
use assert_cmd::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    repo: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("acre");
        let repo = temp.path().join("repo");
        let config = temp.path().join("config.json");
        fs::create_dir_all(&repo).expect("repo dir");
        let settings = serde_json::json!({
            "root": root,
            "pool": { "minSlots": 1, "maxSlots": 2, "replenish": false },
        });
        fs::write(&config, settings.to_string()).expect("config");

        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "acre@example.invalid"]);
        git(&repo, &["config", "user.name", "Acre Tests"]);
        fs::write(repo.join(".gitignore"), "node_modules\n").expect("gitignore");
        fs::write(repo.join("package.json"), "{\"name\":\"fixture\"}\n").expect("package.json");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        Self {
            _temp: temp,
            root,
            repo,
            config,
        }
    }

    fn acre(&self, args: &[&str]) -> Command {
        let mut command = Command::cargo_bin("acre").expect("acre binary");
        command
            .current_dir(&self.repo)
            .env("ACRE_CONFIG", &self.config)
            .env_remove("ACRE_SHELL_SESSION_ID")
            .env_remove("ACRE_DIRECTIVE_FILE")
            .env_remove("ACRE_SHELL_PID")
            .args(args);
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.acre(args).output().expect("run acre");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "expected JSON from acre {args:?}: {error}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    fn open_workspace(&self, branch: &str) -> PathBuf {
        let created = self.json(&["new", branch, "--stay", "--json"]);
        assert_eq!(created["ok"], true, "{created}");
        PathBuf::from(created["path"].as_str().expect("workspace path"))
    }

    fn forget_state(&self) {
        let repositories = self.root.join("repositories");
        for entry in fs::read_dir(&repositories).expect("repositories dir").flatten() {
            let state = entry.path().join("state.json");
            if state.exists() {
                fs::remove_file(&state).expect("remove state");
            }
        }
    }
}

fn git(cwd: &Path, args: &[&str]) {
    run_git(cwd, args).unwrap_or_else(|error| panic!("git {args:?} failed: {error}"));
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    run_git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

#[test]
fn repair_counts_only_records_it_actually_recovered_or_dropped() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("feature/repair");
    let warmed = fixture.json(&["system", "warm", "--json"]);
    assert_eq!(warmed["created"], 1, "{warmed}");

    fixture.forget_state();
    let report = fixture.json(&["system", "repair", "--json"]);
    assert_eq!(report["report"]["addedSlots"], 1, "{report}");
    assert_eq!(report["report"]["addedWorkspaces"], 1, "{report}");
    assert_eq!(report["report"]["removedBrokenRecords"], 0, "{report}");

    let report = fixture.json(&["system", "repair", "--json"]);
    assert_eq!(report["report"]["addedSlots"], 0, "{report}");
    assert_eq!(report["report"]["addedWorkspaces"], 0, "{report}");
    assert_eq!(report["report"]["removedBrokenRecords"], 0, "{report}");

    fs::remove_dir_all(&workspace).expect("delete workspace directory");
    let report = fixture.json(&["system", "repair", "--json"]);
    assert_eq!(report["report"]["addedSlots"], 0, "{report}");
    assert_eq!(report["report"]["addedWorkspaces"], 0, "{report}");
    assert_eq!(report["report"]["removedBrokenRecords"], 1, "{report}");
}

#[test]
fn recovered_workspace_can_be_finished() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("feature/recovered");
    fs::create_dir_all(workspace.join("node_modules/dep")).expect("node_modules");
    fs::write(workspace.join("node_modules/dep/index.js"), "x").expect("dependency file");

    fixture.forget_state();
    let done = fixture.json(&["done", "feature/recovered", "--json"]);
    assert_eq!(done["ok"], true, "{done}");
    assert!(!workspace.exists(), "workspace directory should be released");
    assert!(branch_exists(&fixture.repo, "feature/recovered"));
}

#[test]
fn nested_cache_roots_do_not_block_done() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("feature/nested");
    let nested = workspace.join("packages/app/node_modules/dep");
    fs::create_dir_all(&nested).expect("nested node_modules");
    fs::write(nested.join("index.js"), "x").expect("dependency file");

    let done = fixture.json(&["done", "feature/nested", "--json"]);
    assert_eq!(done["ok"], true, "{done}");
    assert!(branch_exists(&fixture.repo, "feature/nested"));
}

#[test]
fn global_flags_may_precede_the_subcommand() {
    let fixture = Fixture::new();
    let inspected = fixture.json(&["--json", "system", "inspect"]);
    assert_eq!(inspected["ok"], true, "{inspected}");

    fixture
        .acre(&["feature/x", "system", "inspect"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot be combined with a subcommand"));
}
