//! Persistence and lock behavior across real process boundaries.

mod common;

use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use acre::state::lock::RepositoryLock;
use acre::state::storage::write_atomic;
use common::{Fixture, git};
use wait_timeout::ChildExt;

#[test]
fn failed_atomic_replacement_cleans_up_and_preserves_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("state");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), "original").unwrap();
    assert!(write_atomic(&target, b"replacement").is_err());
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "original");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_processes_serialize_updates() {
    let temp = tempfile::tempdir().unwrap();
    let mut children: Vec<_> = (0..4).map(|_| worker(temp.path()).spawn().unwrap()).collect();
    for child in &mut children {
        let status = child.wait_timeout(Duration::from_secs(15)).unwrap();
        if status.is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert!(
            status.is_some_and(|status| status.success()),
            "lock worker failed or timed out"
        );
    }
    assert_eq!(fs::read_to_string(temp.path().join("counter")).unwrap(), "100");
}

#[test]
fn process_exit_releases_the_lock() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = worker(temp.path())
        .env("ACRE_TEST_LOCK_PAUSE", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !temp.path().join("ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        temp.path().join("ready").exists(),
        "child never acquired the lock"
    );
    let _lock = RepositoryLock::acquire(&temp.path().join("state.lock")).unwrap();
}

fn worker(path: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "lock_worker", "--nocapture"])
        .env("ACRE_TEST_LOCK_ROOT", path)
        .env_remove("ACRE_TEST_LOCK_PAUSE")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    command
}

#[test]
fn lock_worker() {
    let Some(path) = std::env::var_os("ACRE_TEST_LOCK_ROOT").map(std::path::PathBuf::from) else {
        return;
    };
    for _ in 0..25 {
        let _lock = RepositoryLock::acquire(&path.join("state.lock")).unwrap();
        if std::env::var_os("ACRE_TEST_LOCK_PAUSE").is_some() {
            fs::write(path.join("ready"), "").unwrap();
            loop {
                std::thread::park();
            }
        }
        let counter = path.join("counter");
        let value = fs::read_to_string(&counter)
            .map(|value| value.parse::<u32>().unwrap())
            .unwrap_or(0);
        std::thread::yield_now();
        fs::write(counter, (value + 1).to_string()).unwrap();
    }
}

#[test]
fn legacy_remote_keyed_state_keeps_its_repository_directory() {
    let fixture = Fixture::new();
    let remote = "https://github.com/example/shared.git";
    git(&fixture.repo, &["remote", "add", "origin", remote]);
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    let id = acre::util::short_hash(remote, 16);
    let legacy = fixture
        .root
        .join("repositories")
        .join(acre::util::repository_slug("shared", &id));
    let remembered = legacy.join("workspaces/remembered");
    let mut state = inspected["state"].clone();
    state["repositoryId"] = serde_json::json!(id);
    state["targetPaths"]["branch:feature"] = serde_json::json!(remembered);
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("state.json"), state.to_string()).unwrap();
    assert!(
        fixture
            .open_workspace("feature")
            .starts_with(legacy.join("workspaces"))
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(legacy.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["repositoryId"], inspected["repository"]["id"]);
}
