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
    temp: TempDir,
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
            // No background replenisher: it would race the test's own view of the pool.
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
            temp,
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
            // Scrub the caller's shell integration so tests behave like a plain terminal.
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

    /// Runs with the shell integration's environment, as the wrapper function would, and returns the
    /// directive file Acre writes its `cd` or resume instruction into.
    fn acre_in_shell(&self, args: &[&str], session: &str, cwd: &Path) -> (Command, PathBuf) {
        let directive = self.temp.path().join(format!("directive-{session}"));
        let mut command = self.acre(args);
        command
            .current_dir(cwd)
            .env("ACRE_SHELL_SESSION_ID", session)
            .env("ACRE_DIRECTIVE_FILE", &directive)
            .env("ACRE_SHELL_PID", std::process::id().to_string());
        (command, directive)
    }

    fn open_workspace(&self, branch: &str) -> PathBuf {
        let created = self.json(&["new", branch, "--stay", "--json"]);
        assert_eq!(created["ok"], true, "{created}");
        PathBuf::from(created["path"].as_str().expect("workspace path"))
    }

    // Simulates lost metadata: the worktrees survive, the state file does not.
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

/// The NUL-separated fields of a directive file: version, action, path, token.
fn directive_fields(path: &Path) -> Vec<String> {
    let bytes = fs::read(path).expect("directive file");
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

#[test]
fn shell_integration_moves_the_shell_and_back_again() {
    let fixture = Fixture::new();
    let (mut new, directive) = fixture.acre_in_shell(&["new", "feature/nav"], "shell-a", &fixture.repo);
    new.assert().success();
    let fields = directive_fields(&directive);
    assert_eq!(fields[0], "acre-directive-v1");
    assert_eq!(fields[1], "cd");
    let workspace = PathBuf::from(&fields[2]);
    assert!(workspace.join("package.json").exists(), "{fields:?}");

    // The shell has followed the directive; `acre -` from there should point back at the repository.
    let (mut back, directive) = fixture.acre_in_shell(&["-"], "shell-a", &workspace);
    back.assert().success();
    let fields = directive_fields(&directive);
    assert_eq!(fields[1], "cd");
    assert_eq!(
        fs::canonicalize(&fields[2]).expect("previous"),
        fs::canonicalize(&fixture.repo).expect("repo")
    );
}

#[test]
fn done_inside_the_workspace_finishes_after_the_shell_moves_out() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("feature/inside");

    let (mut done, directive) = fixture.acre_in_shell(&["done"], "shell-b", &workspace);
    let output = done.output().expect("run done");
    assert_eq!(
        output.status.code(),
        Some(194),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let fields = directive_fields(&directive);
    assert_eq!(fields[1], "resume-after-cd");
    let destination = PathBuf::from(&fields[2]);
    let token = fields[3].clone();
    assert!(destination.is_dir() && !token.is_empty(), "{fields:?}");
    assert!(workspace.exists(), "nothing moves until the shell has left");

    let (mut resume, _) = fixture.acre_in_shell(&["__resume", &token], "shell-b", &destination);
    resume.assert().success();
    assert!(
        !workspace.exists(),
        "the workspace returns to the pool once the shell is out"
    );
    assert!(branch_exists(&fixture.repo, "feature/inside"));
}

#[test]
fn machine_api_acquires_and_releases_a_lease() {
    let fixture = Fixture::new();
    let acquired = fixture.json(&[
        "--json",
        "acquire",
        "feature/agent",
        "--new",
        "--holder",
        "agent:test",
    ]);
    assert_eq!(acquired["ok"], true, "{acquired}");
    let lease_id = acquired["lease_id"].as_str().expect("lease id").to_owned();
    let path = PathBuf::from(acquired["path"].as_str().expect("path"));
    assert!(path.exists());

    let inspected = fixture.json(&["--json", "system", "inspect"]);
    assert_eq!(
        inspected["state"]["leases"].as_array().map(Vec::len),
        Some(1),
        "{inspected}"
    );

    let released = fixture.json(&["--json", "release", "--lease-id", &lease_id]);
    assert_eq!(released["ok"], true, "{released}");
    assert_eq!(released["retained"], false, "{released}");
    assert!(!path.exists(), "the last lease out returns the workspace");
}

#[test]
fn returned_caches_warm_the_next_branch() {
    let fixture = Fixture::new();
    let first = fixture.open_workspace("feature/first");
    fs::create_dir_all(first.join("node_modules/dep")).expect("node_modules");
    fs::write(first.join("node_modules/dep/index.js"), "x").expect("dependency file");
    let done = fixture.json(&["done", "feature/first", "--json"]);
    assert_eq!(done["pooled"], true, "{done}");

    let second = fixture.json(&["new", "feature/second", "--stay", "--json"]);
    assert_eq!(second["environment"]["state"], "ready", "{second}");
    assert_eq!(second["reused"], true, "{second}");
    let path = PathBuf::from(second["path"].as_str().expect("path"));
    assert!(
        path.join("node_modules/dep/index.js").exists(),
        "the pooled caches came along"
    );
}
