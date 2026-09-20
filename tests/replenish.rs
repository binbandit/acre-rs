//! Foreground commands must remain usable while a replenisher prepares private cache copies.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::Fixture;
use wait_timeout::ChildExt;

struct PausedReplenisher {
    child: Child,
    gate: PathBuf,
    slot: PathBuf,
}

impl PausedReplenisher {
    fn start(fixture: &Fixture) -> Self {
        fs::create_dir_all(fixture.repo.join("node_modules/dep")).unwrap();
        fs::write(fixture.repo.join("node_modules/dep/index.js"), "prepared").unwrap();
        let bin = fixture.temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap()).collect();
        let git = paths
            .iter()
            .map(|path| path.join("git"))
            .find(|path| path.is_file())
            .unwrap();
        let gate = fixture.temp.path().join("continue");
        let marker = fixture.temp.path().join("copy-starting");
        let wrapper = bin.join("git");
        fs::write(&wrapper, "#!/bin/sh\nfor arg do\n  if [ \"$arg\" = check-ignore ]; then\n    pwd > \"$ACRE_TEST_MARKER\"\n    while [ ! -e \"$ACRE_TEST_GATE\" ]; do sleep 0.02; done\n  fi\ndone\nexec \"$ACRE_TEST_GIT\" \"$@\"\n").unwrap();
        fs::set_permissions(wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let child = fixture
            .acre(&["__replenish", fixture.repo.join(".git").to_str().unwrap()])
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(bin).chain(paths)).unwrap(),
            )
            .env("ACRE_TEST_GIT", git)
            .env("ACRE_TEST_MARKER", &marker)
            .env("ACRE_TEST_GATE", &gate)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut paused = Self {
            child,
            gate,
            slot: PathBuf::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        paused.slot = PathBuf::from(
            fs::read_to_string(marker)
                .expect("replenisher reached cache copy")
                .trim(),
        );
        paused
    }

    fn finish(&mut self) {
        fs::write(&self.gate, "").unwrap();
        assert!(
            self.child
                .wait_timeout(Duration::from_secs(10))
                .unwrap()
                .is_some_and(|status| status.success())
        );
    }
}

impl Drop for PausedReplenisher {
    fn drop(&mut self) {
        // Unblock the real Git child even if an assertion fails before finish().
        let _ = fs::write(&self.gate, "");
        if self
            .child
            .wait_timeout(Duration::from_secs(3))
            .ok()
            .flatten()
            .is_none()
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn replenishment_does_not_block_foreground_commands_or_overwrite_their_state() {
    let fixture = Fixture::new();
    let mut warming = PausedReplenisher::start(&fixture);
    assert_cmd::Command::from(fixture.acre(&["new", "foreground", "--stay", "--json"]))
        .timeout(Duration::from_secs(3))
        .assert()
        .success();
    assert_cmd::Command::from(fixture.acre(&["main", "--json"]))
        .timeout(Duration::from_secs(3))
        .assert()
        .success();
    warming.finish();
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert!(
        inspected["state"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["target"]["localBranch"] == "foreground")
    );
    assert_eq!(
        fs::read_to_string(warming.slot.join("node_modules/dep/index.js")).unwrap(),
        "prepared"
    );
    let warmed = inspected["state"]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| {
            fs::canonicalize(slot["path"].as_str().unwrap()).unwrap()
                == fs::canonicalize(&warming.slot).unwrap()
        })
        .unwrap();
    assert_eq!(warmed["status"], "idle");
}

#[test]
fn replenishment_preserves_a_worktree_adopted_during_preparation() {
    let fixture = Fixture::new();
    let mut warming = PausedReplenisher::start(&fixture);
    assert_cmd::Command::from(fixture.acre(&[warming.slot.to_str().unwrap(), "--json"]))
        .timeout(Duration::from_secs(3))
        .assert()
        .success();
    fs::create_dir_all(warming.slot.join("node_modules/dep")).unwrap();
    fs::write(
        warming.slot.join("node_modules/dep/index.js"),
        "user installation",
    )
    .unwrap();
    warming.finish();
    assert_eq!(
        fs::read_to_string(warming.slot.join("node_modules/dep/index.js")).unwrap(),
        "user installation"
    );
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert_eq!(inspected["state"]["slots"][0]["status"], "active");
}

#[test]
fn replenishment_preserves_manual_edits_during_preparation() {
    let fixture = Fixture::new();
    let mut warming = PausedReplenisher::start(&fixture);
    fs::write(warming.slot.join("package.json"), "user changes").unwrap();
    warming.finish();
    assert_eq!(
        fs::read_to_string(warming.slot.join("package.json")).unwrap(),
        "user changes"
    );
    assert!(!warming.slot.join("node_modules").exists());
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert_eq!(inspected["state"]["slots"][0]["status"], "retained");
}

#[test]
fn replenishment_discards_a_source_that_changes_during_copying() {
    let fixture = Fixture::new();
    let mut warming = PausedReplenisher::start(&fixture);
    fs::write(
        fixture.repo.join("package.json"),
        "{\"dependencies\":{\"changed\":\"2\"}}",
    )
    .unwrap();
    fs::write(fixture.repo.join("node_modules/dep/index.js"), "incompatible").unwrap();
    warming.finish();
    assert!(!warming.slot.join("node_modules").exists());
    let inspected = fixture.json(&["--json", "system", "inspect"]);
    let slot = inspected["state"]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| {
            fs::canonicalize(slot["path"].as_str().unwrap()).unwrap()
                == fs::canonicalize(&warming.slot).unwrap()
        })
        .unwrap();
    assert_eq!(slot["environment"]["state"], "cold");
}
