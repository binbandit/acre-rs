//! Isolated repositories and CLI helpers shared by integration tests.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

pub struct Fixture {
    pub temp: TempDir,
    pub root: PathBuf,
    pub repo: PathBuf,
    pub config: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
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

    pub fn acre(&self, args: &[&str]) -> Command {
        let mut command = Command::cargo_bin("acre").expect("acre binary");
        command
            .current_dir(&self.repo)
            .env("ACRE_CONFIG", &self.config)
            // Scrub the caller's shell integration so tests behave like a plain terminal.
            .env_remove("ACRE_SHELL_SESSION_ID")
            .env_remove("ACRE_DIRECTIVE_FILE")
            .env_remove("ACRE_SHELL_PID")
            .args(args);
        isolate_git(&mut command);
        command
    }

    pub fn json(&self, args: &[&str]) -> Value {
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
    pub fn acre_in_shell(&self, args: &[&str], session: &str, cwd: &Path) -> (Command, PathBuf) {
        let directive = self.temp.path().join(format!("directive-{session}"));
        let mut command = self.acre(args);
        command
            .current_dir(cwd)
            .env("ACRE_SHELL_SESSION_ID", session)
            .env("ACRE_DIRECTIVE_FILE", &directive)
            .env("ACRE_SHELL_PID", std::process::id().to_string());
        (command, directive)
    }

    pub fn open_workspace(&self, branch: &str) -> PathBuf {
        let created = self.json(&["new", branch, "--stay", "--json"]);
        assert_eq!(created["ok"], true, "{created}");
        PathBuf::from(created["path"].as_str().expect("workspace path"))
    }

    // Simulates lost metadata: the worktrees survive, the state file does not.
    pub fn forget_state(&self) {
        let repositories = self.root.join("repositories");
        for entry in fs::read_dir(&repositories).expect("repositories dir").flatten() {
            let state = entry.path().join("state.json");
            if state.exists() {
                fs::remove_file(&state).expect("remove state");
            }
        }
    }
}

/// Keeps the developer's Git setup out of the tests: no global or system config (signing,
/// excludes, default branch), and no `GIT_DIR` and friends exported by a calling hook.
pub fn isolate_git(command: &mut Command) -> &mut Command {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
}

pub fn git(cwd: &Path, args: &[&str]) {
    let output = isolate_git(Command::new("git").current_dir(cwd).args(args))
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    isolate_git(Command::new("git").current_dir(repo).args([
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ]))
    .status()
    .expect("run git")
    .success()
}
