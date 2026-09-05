//! Regression coverage for leases, cache trust, and filesystem boundaries through the CLI.

mod common;

use std::fs;
use std::path::PathBuf;

use assert_cmd::prelude::*;
use common::{Fixture, git};
use serde_json::json;

#[test]
fn commits_made_in_an_idle_slot_are_not_recycled_or_collected() {
    let fixture = Fixture::new();
    let warmed = fixture.json(&["system", "warm", "--json"]);
    let path = PathBuf::from(warmed["slots"][0]["path"].as_str().unwrap());
    fs::write(path.join("work.txt"), "committed work").unwrap();
    git(&path, &["add", "work.txt"]);
    git(&path, &["commit", "-qm", "keep this detached commit"]);
    let mut config: serde_json::Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["pool"]["idleRetentionDays"] = json!(0);
    fs::write(&fixture.config, config.to_string()).unwrap();
    let gc = fixture.json(&["system", "gc", "--json"]);
    assert_eq!(gc["removed"], json!([]), "{gc}");
    assert_ne!(fixture.open_workspace("next"), path);
    assert_eq!(
        fs::read_to_string(path.join("work.txt")).unwrap(),
        "committed work"
    );
    assert_eq!(fixture.json(&[path.to_str().unwrap(), "--json"])["ok"], true);
    let done = fixture.json(&["done", path.to_str().unwrap(), "--json"]);
    assert_eq!(done["retained"], true, "{done}");
    assert_eq!(done["assessment"]["detachedCommits"], true);
}

#[test]
fn changed_source_manifests_prevent_cache_copying() {
    for (primary, committed) in [(true, false), (false, false), (false, true)] {
        let fixture = Fixture::new();
        let source = if primary {
            fixture.repo.clone()
        } else {
            fixture.open_workspace("source")
        };
        fs::create_dir(source.join("node_modules")).unwrap();
        fs::write(source.join("node_modules/generation"), "incompatible").unwrap();
        fs::write(source.join("package.json"), r#"{"dependencies":{"example":"2"}}"#).unwrap();
        if committed {
            git(&source, &["commit", "-am", "different generation"]);
        }
        let opened = fixture.json(&["new", "destination", "--stay", "--json"]);
        assert_eq!(opened["ok"], true, "{opened}");
        assert_eq!(opened["environment"]["state"], "cold", "{opened}");
        let destination = PathBuf::from(opened["path"].as_str().unwrap());
        assert!(!destination.join("node_modules/generation").exists());
    }
}

#[cfg(unix)]
#[test]
fn activation_does_not_change_caches_when_state_cannot_be_saved() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    fs::create_dir(fixture.repo.join("node_modules")).unwrap();
    fs::write(fixture.repo.join("node_modules/generation"), "old").unwrap();
    let path = fixture.open_workspace("first");
    assert_eq!(fixture.json(&["done", "first", "--json"])["pooled"], true);
    fs::write(
        fixture.repo.join("package.json"),
        r#"{"dependencies":{"example":"2"}}"#,
    )
    .unwrap();
    git(&fixture.repo, &["commit", "-am", "new generation"]);
    fs::write(fixture.repo.join("node_modules/generation"), "new").unwrap();

    let state_directory = path.parent().unwrap().parent().unwrap();
    let permissions = fs::metadata(state_directory).unwrap().permissions();
    fs::set_permissions(state_directory, fs::Permissions::from_mode(0o500)).unwrap();
    let opened = fixture.json(&["new", "second", "--stay", "--json"]);
    fs::set_permissions(state_directory, permissions).unwrap();
    assert_eq!(opened["ok"], false, "{opened}");
    assert_eq!(
        fs::read_to_string(path.join("node_modules/generation")).unwrap(),
        "old"
    );
}

#[test]
fn opening_an_idle_workspace_by_path_takes_it_out_of_the_pool() {
    let fixture = Fixture::new();
    let path = fixture.open_workspace("first");
    assert_eq!(fixture.json(&["done", "first", "--json"])["pooled"], true);
    let opened = fixture.json(&[path.to_str().unwrap(), "--json"]);
    assert_eq!(opened["ownership"], "acre", "{opened}");
    assert_ne!(fixture.open_workspace("second"), path);
}

#[cfg(unix)]
#[test]
fn failed_activation_cannot_publish_new_caches_under_an_old_fingerprint() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join(".gitignore"), "node_modules\n.env\n").unwrap();
    git(&fixture.repo, &["commit", "-am", "ignore seed"]);
    fs::create_dir(fixture.repo.join("node_modules")).unwrap();
    fs::write(fixture.repo.join("node_modules/generation"), "old").unwrap();
    let path = fixture.open_workspace("first");
    assert_eq!(fixture.json(&["done", "first", "--json"])["pooled"], true);
    fs::write(
        fixture.repo.join("package.json"),
        r#"{"dependencies":{"example":"2"}}"#,
    )
    .unwrap();
    git(&fixture.repo, &["commit", "-am", "new generation"]);
    fs::write(fixture.repo.join("node_modules/generation"), "new").unwrap();

    // A looping seed link fails inspection after the new caches have been copied.
    std::os::unix::fs::symlink(".env", fixture.repo.join(".env")).unwrap();
    let opened = fixture.json(&["new", "second", "--stay", "--json"]);
    assert_eq!(opened["ok"], false, "{opened}");
    assert_eq!(
        fs::read_to_string(path.join("node_modules/generation")).unwrap(),
        "new"
    );
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert_eq!(inspected["state"]["slots"], json!([]));
    assert_eq!(inspected["state"]["workspaces"][0]["status"], "retained");
    assert!(inspected["state"]["workspaces"][0]["environment"].is_null());
    fs::remove_file(fixture.repo.join(".env")).unwrap();
    let reopened = fixture.json(&["first", "--json"]);
    assert_eq!(reopened["ok"], true, "{reopened}");
    assert_ne!(reopened["path"], path.to_str().unwrap());
    assert_eq!(reopened["environment"]["state"], "cold", "{reopened}");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn idle_workspaces_with_running_processes_are_not_recycled() {
    let fixture = Fixture::new();
    let path = fixture.open_workspace("first");
    assert_eq!(fixture.json(&["done", "first", "--json"])["pooled"], true);
    let mut process = std::process::Command::new("sleep")
        .arg("10")
        .current_dir(&path)
        .spawn()
        .unwrap();
    let opened = fixture.json(&["new", "second", "--stay", "--json"]);
    let _ = process.kill();
    process.wait().unwrap();
    assert_eq!(opened["ok"], true, "{opened}");
    assert_ne!(opened["path"], path.to_str().unwrap());
}

#[test]
fn done_respects_machine_leases_without_a_shell_session() {
    let fixture = Fixture::new();
    let acquired = fixture.json(&["acquire", "held", "--new", "--holder", "editor", "--json"]);
    let path = PathBuf::from(acquired["path"].as_str().expect("workspace path"));
    let done = fixture.json(&["done", "held", "--json"]);
    assert_eq!(done["retained"], true, "{done}");
    assert_eq!(done["assessment"]["leases"][0]["holder"], "editor");
    assert!(path.exists());
}

#[test]
fn done_retains_new_detached_commits() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("review");
    git(&workspace, &["switch", "--detach"]);
    fs::write(workspace.join("work.txt"), "new work").unwrap();
    git(&workspace, &["add", "work.txt"]);
    git(&workspace, &["commit", "-m", "detached work"]);
    let done = fixture.json(&["done", "review", "--json"]);
    assert_eq!(done["retained"], true, "{done}");
    assert!(workspace.join("work.txt").exists());
}

#[test]
fn recycling_a_dirty_idle_slot_preserves_user_changes() {
    let fixture = Fixture::new();
    let warmed = fixture.json(&["system", "warm", "--json"]);
    let slot = PathBuf::from(warmed["slots"][0]["path"].as_str().expect("slot path"));
    fs::write(slot.join("package.json"), "uncommitted work").unwrap();
    fs::write(
        fixture.repo.join("package.json"),
        "{\"dependencies\":{\"new\":\"1\"}}",
    )
    .unwrap();
    git(&fixture.repo, &["commit", "-am", "new generation"]);
    fixture.open_workspace("next");
    assert_eq!(
        fs::read_to_string(slot.join("package.json")).unwrap(),
        "uncommitted work"
    );
}

#[test]
fn locked_idle_slots_are_not_moved_or_recycled() {
    let fixture = Fixture::new();
    let warmed = fixture.json(&["system", "warm", "--json"]);
    let slot = PathBuf::from(warmed["slots"][0]["path"].as_str().expect("slot path"));
    git(&fixture.repo, &["worktree", "lock", slot.to_str().unwrap()]);
    fixture.open_workspace("next");
    assert!(
        slot.join("package.json").exists(),
        "locked slot must stay in place"
    );
}

#[test]
fn worktree_selector_does_not_fall_back_to_creating_a_branch_workspace() {
    let fixture = Fixture::new();
    git(&fixture.repo, &["branch", "unopened"]);
    fixture.acre(&["worktree:unopened", "--json"]).assert().failure();
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert!(inspected["state"]["workspaces"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn cache_cleanup_does_not_follow_a_symlinked_parent() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside");
    fs::create_dir_all(outside.join("cache")).unwrap();
    fs::write(outside.join("cache/keep"), "irreplaceable").unwrap();
    std::os::unix::fs::symlink(&outside, fixture.repo.join(".next")).unwrap();
    fs::write(
        fixture.repo.join(".acre.json"),
        json!({
            "environment": { "cacheRoots": [".next/cache"] }
        })
        .to_string(),
    )
    .unwrap();
    git(&fixture.repo, &["add", ".next", ".acre.json"]);
    git(&fixture.repo, &["commit", "-m", "linked cache parent"]);
    fixture.open_workspace("next");
    assert_eq!(
        fs::read_to_string(outside.join("cache/keep")).unwrap(),
        "irreplaceable"
    );
}

#[cfg(unix)]
#[test]
fn seed_copy_does_not_follow_a_dangling_destination_symlink() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside-secret");
    fs::write(fixture.repo.join(".gitignore"), "node_modules\n.env\n").unwrap();
    git(&fixture.repo, &["commit", "-am", "ignore secrets"]);
    git(&fixture.repo, &["switch", "-c", "linked"]);
    std::os::unix::fs::symlink(&outside, fixture.repo.join(".env")).unwrap();
    git(&fixture.repo, &["add", "-f", ".env"]);
    git(&fixture.repo, &["commit", "-m", "tracked link"]);
    git(&fixture.repo, &["switch", "main"]);
    fs::write(fixture.repo.join(".env"), "local secret").unwrap();
    fixture
        .acre(&["new", "next", "--from", "linked", "--stay"])
        .assert()
        .success();
    assert!(!outside.exists(), "seed content must never escape the workspace");
}

#[test]
fn active_untrusted_slots_are_never_cache_sources() {
    let fixture = Fixture::new();
    let fork = fixture.open_workspace("fork");
    fs::create_dir_all(fork.join("node_modules")).unwrap();
    fs::write(fork.join("node_modules/poison"), "untrusted cache").unwrap();
    // Simulate the persisted trust record produced by opening a fork PR, without network or gh.
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    let mut state = inspected["state"].clone();
    state["workspaces"][0]["trust"] = json!("untrusted");
    let state_path = fork.parent().unwrap().parent().unwrap().join("state.json");
    fs::write(state_path, state.to_string()).unwrap();
    let trusted = fixture.open_workspace("trusted");
    assert!(!trusted.join("node_modules/poison").exists());
}

#[test]
fn repository_config_rejects_the_worktree_root_as_a_cache_path() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repo.join(".acre.json"),
        r#"{"environment":{"cacheRoots":["."]}}"#,
    )
    .unwrap();
    let opened = fixture.json(&["new", "next", "--stay", "--json"]);
    assert_eq!(opened["error"]["code"], "ACRE_CONFIG_INVALID", "{opened}");
}

#[test]
fn gc_preserves_unknown_ignored_files_in_idle_slots() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join(".gitignore"), "node_modules\n*.bak\n").unwrap();
    git(&fixture.repo, &["commit", "-am", "ignore backups"]);
    let warmed = fixture.json(&["system", "warm", "--json"]);
    let slot = PathBuf::from(warmed["slots"][0]["path"].as_str().unwrap());
    fs::write(slot.join("notes.bak"), "user backup").unwrap();
    fixture
        .acre(&["config", "set", "pool.idleRetentionDays", "0"])
        .assert()
        .success();
    let gc = fixture.json(&["system", "gc", "--json"]);
    assert!(gc["removed"].as_array().unwrap().is_empty(), "{gc}");
    assert_eq!(fs::read_to_string(slot.join("notes.bak")).unwrap(), "user backup");
}

#[test]
fn excluded_roots_are_normalized_like_cache_roots() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repo.join(".acre.json"),
        r#"{"environment":{"excludedRoots":["./node_modules/"]}}"#,
    )
    .unwrap();
    let opened = fixture.json(&["new", "next", "--stay", "--json"]);
    assert!(
        opened["environment"]["cacheRoots"].as_array().unwrap().is_empty(),
        "{opened}"
    );
}

#[test]
fn repository_config_always_comes_from_the_primary_checkout() {
    let fixture = Fixture::new();
    let first = fixture.open_workspace("first");
    fs::write(
        fixture.repo.join(".acre.json"),
        r#"{"environment":{"excludedRoots":["node_modules"]}}"#,
    )
    .unwrap();
    let output = fixture
        .acre(&["new", "second", "--stay", "--json"])
        .current_dir(first)
        .output()
        .unwrap();
    let opened: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        opened["environment"]["cacheRoots"].as_array().unwrap().is_empty(),
        "{opened}"
    );
}

#[cfg(unix)]
#[test]
fn state_writes_do_not_change_existing_parent_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::set_permissions(fixture.temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fixture
        .acre(&["config", "set", "pool.replenish", "false"])
        .assert()
        .success();
    assert_eq!(
        fs::metadata(fixture.temp.path()).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn clones_of_the_same_remote_have_independent_state() {
    let first = Fixture::new();
    let second = Fixture::new();
    for fixture in [&first, &second] {
        git(
            &fixture.repo,
            &["remote", "add", "origin", "https://github.com/example/shared.git"],
        );
    }
    let first_path = first.open_workspace("feature");
    let output = second
        .acre(&["new", "feature", "--stay", "--json"])
        .env("ACRE_CONFIG", &first.config)
        .output()
        .unwrap();
    let opened: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(opened["ok"], true, "{opened}");
    let second_path = PathBuf::from(opened["path"].as_str().unwrap());
    assert_ne!(first_path, second_path);
    assert!(first_path.exists() && second_path.exists());
    let inspected = first.json(&["system", "inspect", "--json"]);
    assert_eq!(
        inspected["state"]["workspaces"].as_array().unwrap().len(),
        1,
        "each clone must see only its own workspace records: {inspected}"
    );
}

#[test]
fn package_manager_detection_does_not_depend_on_json_whitespace() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repo.join("package.json"),
        r#"{"packageManager":"yarn@4.0.0"}"#,
    )
    .unwrap();
    git(&fixture.repo, &["commit", "-am", "compact manifest"]);
    let first = fixture.json(&["new", "first", "--stay", "--json"]);
    fs::write(
        fixture.repo.join("package.json"),
        "{\n  \"packageManager\": \"yarn@4.0.0\"\n}\n",
    )
    .unwrap();
    git(&fixture.repo, &["commit", "-am", "pretty manifest"]);
    let second = fixture.json(&["new", "second", "--stay", "--json"]);
    assert_eq!(
        first["environment"]["fingerprint"],
        second["environment"]["fingerprint"]
    );
}

#[test]
fn nested_dependency_changes_invalidate_the_environment() {
    let fixture = Fixture::new();
    let package = fixture.repo.join("packages/app");
    fs::create_dir_all(&package).unwrap();
    fs::write(package.join("package.json"), r#"{"dependencies":{"a":"1"}}"#).unwrap();
    git(&fixture.repo, &["add", "-A"]);
    git(&fixture.repo, &["commit", "-m", "workspace package"]);
    let first = fixture.json(&["new", "first", "--stay", "--json"]);
    fs::write(package.join("package.json"), r#"{"dependencies":{"a":"2"}}"#).unwrap();
    git(&fixture.repo, &["commit", "-am", "nested dependency update"]);
    let second = fixture.json(&["new", "second", "--stay", "--json"]);
    assert_ne!(
        first["environment"]["fingerprint"],
        second["environment"]["fingerprint"]
    );
}

#[cfg(unix)]
#[test]
fn opening_a_workspace_does_not_execute_checkout_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let marker = fixture.temp.path().join("hook-ran");
    let hook = fixture.repo.join(".git/hooks/post-checkout");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\n", acre::util::shell_quote(&marker)),
    )
    .unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.open_workspace("next");
    assert!(
        !marker.exists(),
        "Git checkout hooks must not execute during workspace preparation"
    );
}

#[test]
fn shell_session_ids_cannot_escape_the_state_directory() {
    let fixture = Fixture::new();
    let sentinel = fixture.root.join("sentinel.json");
    fs::create_dir_all(&fixture.root).unwrap();
    fs::write(&sentinel, "{}").unwrap();
    let (mut command, _) = fixture.acre_in_shell(&["new", "next"], "../sentinel", &fixture.repo);
    command.env("ACRE_DIRECTIVE_FILE", fixture.temp.path().join("directive"));
    command.assert().failure();
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "{}");
}

#[cfg(unix)]
#[test]
fn missing_pull_request_trust_metadata_withholds_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::write(fixture.repo.join(".gitignore"), "node_modules\n.env\n").unwrap();
    git(&fixture.repo, &["commit", "-am", "ignore secrets"]);
    fs::write(fixture.repo.join(".env"), "secret").unwrap();
    git(
        &fixture.repo,
        &["remote", "add", "origin", "https://github.com/example/shared.git"],
    );
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    let response = json!({
        "number": 1, "title": "Review", "url": "https://github.com/example/shared/pull/1",
        "baseRefName": "main", "headRefName": "feature",
        "headRefOid": inspected["repository"]["currentWorktree"]["head"]
    });
    let bin = fixture.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(
        bin.join("gh"),
        format!("#!/bin/sh\ncat <<'JSON'\n{response}\nJSON\n"),
    )
    .unwrap();
    fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
    let paths = std::iter::once(bin)
        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))
        .collect::<Vec<_>>();
    let output = fixture
        .acre(&["pr:1", "--json"])
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    let opened: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(opened["ok"], true, "{opened}");
    let path = PathBuf::from(opened["path"].as_str().unwrap());
    assert!(
        !path.join(".env").exists(),
        "unknown trust must not receive secrets"
    );
}
