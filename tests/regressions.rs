//! CLI regressions for the independently reproduced audit failures.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Fixture, branch_exists, git};
use serde_json::{Value, json};

fn configure(fixture: &Fixture, key: &str, value: Value) {
    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config[key] = value;
    fs::write(&fixture.config, config.to_string()).unwrap();
}

#[test]
fn failed_remote_activation_preserves_existing_branch() {
    let f = Fixture::new();
    git(&f.repo, &["remote", "add", "origin", f.repo.to_str().unwrap()]);
    git(&f.repo, &["branch", "feature"]);
    git(&f.repo, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
    let opened = f.json(&["--json", "remote:origin/feature"]);
    assert_eq!(opened["ok"], false);
    assert!(branch_exists(&f.repo, "feature"));
}

#[test]
fn ignored_siblings_of_seeds_are_preserved() {
    let f = Fixture::new();
    configure(&f, "environment", json!({"seedFiles": ["local/.env"]}));
    fs::write(f.repo.join(".gitignore"), "node_modules/\nlocal/\n").unwrap();
    git(&f.repo, &["commit", "-am", "ignore local files"]);
    fs::create_dir(f.repo.join("local")).unwrap();
    fs::write(f.repo.join("local/.env"), "synthetic secret").unwrap();
    let workspace = f.open_workspace("work");
    let notes = workspace.join("local/notes.txt");
    fs::write(&notes, "only copy").unwrap();
    f.json(&["--json", "system", "warm", "--slots", "2"]);
    let result = f.json(&["--json", "done", "work"]);
    assert_eq!(result["retained"], true, "{result}");
    assert_eq!(fs::read_to_string(notes).unwrap(), "only copy");
    assert!(
        result["assessment"]["newIgnored"]
            .as_array()
            .unwrap()
            .contains(&json!("local/notes.txt"))
    );
}

#[test]
fn cache_roots_cannot_contain_seed_secrets() {
    let f = Fixture::new();
    configure(
        &f,
        "environment",
        json!({"seedFiles": ["local/.env"], "cacheRoots": ["local"]}),
    );
    let result = f.json(&["--json", "new", "work"]);
    assert_eq!(result["error"]["code"], "ACRE_CONFIG_INVALID", "{result}");
    assert!(!branch_exists(&f.repo, "work"));
}

#[test]
fn directory_seeds_use_normalized_paths_and_directory_ignore_rules() {
    let f = Fixture::new();
    configure(&f, "environment", json!({"seedFiles": [".//local/./"]}));
    fs::write(f.repo.join(".gitignore"), "node_modules/\nlocal/\n").unwrap();
    git(&f.repo, &["commit", "-am", "ignore local directories"]);
    fs::create_dir(f.repo.join("local")).unwrap();
    fs::write(f.repo.join("local/secret"), "synthetic seed").unwrap();
    let workspace = f.open_workspace("work");
    assert_eq!(
        fs::read_to_string(workspace.join("local/secret")).unwrap(),
        "synthetic seed"
    );
    assert_eq!(f.json(&["--json", "done", "work"])["ok"], true);
    assert!(!workspace.join("local").exists());
    assert_eq!(
        fs::read_to_string(f.repo.join("local/secret")).unwrap(),
        "synthetic seed"
    );
}

#[test]
fn manual_branch_switch_does_not_redirect_original_selector() {
    let f = Fixture::new();
    let workspace = f.open_workspace("old");
    git(&workspace, &["switch", "-c", "different"]);
    let opened = f.json(&["--json", "old"]);
    assert_eq!(opened["ok"], true, "{opened}");
    assert_ne!(PathBuf::from(opened["path"].as_str().unwrap()), workspace);
    let actual =
        acre::git::repository::discover_repository(&PathBuf::from(opened["path"].as_str().unwrap())).unwrap();
    assert_eq!(actual.current_worktree.unwrap().branch.as_deref(), Some("old"));
    assert_eq!(
        acre::git::repository::discover_repository(&workspace)
            .unwrap()
            .current_worktree
            .unwrap()
            .branch
            .as_deref(),
        Some("different")
    );
}

#[test]
fn remote_name_changes_keep_existing_leases_discoverable() {
    let f = Fixture::new();
    let acquired = f.json(&["--json", "acquire", "work", "--new", "--holder", "test"]);
    git(
        &f.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/new-name.git",
        ],
    );
    assert_eq!(
        f.json(&["--json", "system", "inspect"])["state"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let result = f.json(&[
        "--json",
        "release",
        "--lease-id",
        acquired["lease_id"].as_str().unwrap(),
    ]);
    assert_eq!(result["ok"], true, "{result}");
}

#[test]
fn relative_roots_stay_relative_to_configuration() {
    let f = Fixture::new();
    configure(&f, "root", json!("relative-acre"));
    let acquired = f.json(&["--json", "acquire", "work", "--new", "--holder", "test"]);
    let path = PathBuf::from(acquired["path"].as_str().unwrap());
    let output = f
        .acre(&[
            "--json",
            "release",
            "--keep-active",
            "--lease-id",
            acquired["lease_id"].as_str().unwrap(),
        ])
        .current_dir(path)
        .output()
        .unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{result}");
}

#[test]
fn index_failures_do_not_issue_unreleaseable_leases() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("repositories.json")).unwrap();
    let result = f.json(&["--json", "acquire", "work", "--new", "--holder", "test"]);
    assert_eq!(result["ok"], false);
    assert!(!branch_exists(&f.repo, "work"));
    fs::remove_dir(f.root.join("repositories.json")).unwrap();
    assert!(
        f.json(&["--json", "system", "inspect"])["state"]["leases"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn valid_branch_names_are_not_provider_or_symbolic_refs() {
    let f = Fixture::new();
    for name in ["feature/HEAD", "feature/pull/7"] {
        git(&f.repo, &["branch", name]);
        let result = f.json(&["--json", name]);
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["branch"], name);
    }
}

#[test]
fn foreign_pull_request_urls_fail_before_provider_lookup() {
    let f = Fixture::new();
    git(
        &f.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/current/project.git",
        ],
    );
    let result = f.json(&["--json", "https://github.com/other/project/pull/7"]);
    assert_eq!(result["error"]["code"], "ACRE_PR_REPOSITORY_MISMATCH");
}

#[test]
fn default_branch_configuration_is_honored() {
    let f = Fixture::new();
    git(&f.repo, &["config", "init.defaultBranch", "main"]);
    git(&f.repo, &["switch", "-c", "feature"]);
    fs::write(f.repo.join("feature-only"), "not a default").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-qm", "feature"]);
    let result = f.json(&["--json", "new", "work", "--stay"]);
    assert_eq!(result["base_ref"], "main");
    assert!(
        !PathBuf::from(result["path"].as_str().unwrap())
            .join("feature-only")
            .exists()
    );
}

#[test]
fn directory_override_applies_to_relative_worktree_selectors() {
    let f = Fixture::new();
    let output = f
        .acre(&["--json", "-C", f.repo.to_str().unwrap(), "worktree:."])
        .current_dir(f.temp.path())
        .output()
        .unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{result}");
    assert_eq!(
        fs::canonicalize(result["path"].as_str().unwrap()).unwrap(),
        fs::canonicalize(&f.repo).unwrap()
    );
}

#[test]
fn done_uses_the_shell_directory_even_with_a_directory_override() {
    let f = Fixture::new();
    configure(
        &f,
        "pool",
        json!({"minSlots": 1, "maxSlots": 1, "replenish": false}),
    );
    let workspace = f.open_workspace("work");
    assert_eq!(f.json(&["--json", "system", "warm"])["ok"], true);
    let args = ["-C", f.repo.to_str().unwrap(), "done", "work"];

    let output = f
        .acre(&args)
        .arg("--json")
        .current_dir(&workspace)
        .output()
        .unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["error"]["code"], "ACRE_SHELL_MOVE_REQUIRED", "{result}");
    assert!(workspace.exists());

    let (mut command, directive) = f.acre_in_shell(&args, "override", &workspace);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(194), "{output:?}");
    assert!(workspace.exists(), "the shell has not moved yet");
    let bytes = fs::read(directive).unwrap();
    let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    let destination = PathBuf::from(std::str::from_utf8(fields[2]).unwrap());
    let token = std::str::from_utf8(fields[3]).unwrap();
    let (mut command, _) = f.acre_in_shell(&["__resume", token], "override", &destination);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        !workspace.exists(),
        "only remove the workspace after the shell leaves"
    );
    assert!(branch_exists(&f.repo, "work"));
}

#[test]
fn done_lands_in_the_same_subdirectory_and_forgets_the_pooled_slot() {
    let f = Fixture::new();
    fs::create_dir(f.repo.join("src")).unwrap();
    fs::write(f.repo.join("src/tracked"), "tracked").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-qm", "nested directory"]);
    let workspace = f.open_workspace("work");
    let from = workspace.join("src");
    let (mut command, directive) = f.acre_in_shell(&["done"], "subdirectory", &from);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(194), "{output:?}");
    let bytes = fs::read(directive).unwrap();
    let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    let destination = PathBuf::from(std::str::from_utf8(fields[2]).unwrap());
    assert_eq!(
        fs::canonicalize(&destination).unwrap(),
        fs::canonicalize(f.repo.join("src")).unwrap()
    );
    let token = std::str::from_utf8(fields[3]).unwrap();
    let (mut command, _) = f.acre_in_shell(&["__resume", token], "subdirectory", &destination);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    // The returned directory is an idle slot now; `acre -` must not lead the shell back into it.
    let (mut command, directive) = f.acre_in_shell(&["-"], "subdirectory", &destination);
    let _ = fs::remove_file(&directive);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(!directive.exists());
}

#[test]
fn remote_names_with_slashes_preserve_branch_names() {
    let f = Fixture::new();
    git(
        &f.repo,
        &["remote", "add", "team/upstream", f.repo.to_str().unwrap()],
    );
    git(&f.repo, &["branch", "feature"]);
    git(&f.repo, &["branch", "other"]);
    git(&f.repo, &["fetch", "team/upstream"]);
    git(&f.repo, &["branch", "-D", "feature", "other"]);
    git(&f.repo, &["remote", "set-head", "team/upstream", "main"]);
    for (selector, expected) in [("feature", "feature"), ("remote:team/upstream/other", "other")] {
        let opened = f.json(&["--json", selector]);
        assert_eq!(opened["ok"], true, "{opened}");
        assert_eq!(opened["branch"], expected);
        let actual =
            acre::git::repository::discover_repository(&PathBuf::from(opened["path"].as_str().unwrap()))
                .unwrap();
        assert_eq!(actual.current_worktree.unwrap().branch.as_deref(), Some(expected));
    }
    assert!(
        acre::git::refs::list_refs(&f.repo)
            .unwrap()
            .iter()
            .all(|reference| { reference.qualified_name() != "team/upstream/HEAD" })
    );
}

#[test]
fn done_preserves_active_bisect() {
    let f = Fixture::new();
    let old = acre::git::refs::resolve_oid(&f.repo, "HEAD").unwrap().unwrap();
    fs::write(f.repo.join("next"), "next").unwrap();
    git(&f.repo, &["add", "."]);
    git(&f.repo, &["commit", "-qm", "next"]);
    let workspace = f.open_workspace("work");
    git(&workspace, &["bisect", "start", "--no-checkout", "HEAD", &old]);
    let result = f.json(&["--json", "done", "work"]);
    assert_eq!(result["retained"], true, "{result}");
    assert_eq!(result["assessment"]["operation"], "bisect");
    assert!(workspace.exists());
}

#[test]
fn json_configuration_and_argument_errors_are_documents() {
    let f = Fixture::new();
    for args in [
        vec!["--json", "config", "set", "pool.replenish", "false"],
        vec!["--json", "config", "path"],
        vec!["--json", "config", "repo-init"],
        vec!["--json", "config", "init", "--force"],
        vec!["--json", "shell", "init", "bash"],
        vec!["--json", "completion", "zsh"],
        vec!["--json", "__session-id"],
        vec!["--json", "__complete", "ma"],
        vec!["--json", "--version"],
    ] {
        assert_eq!(f.json(&args)["ok"], true);
    }
    assert_eq!(
        f.json(&["--json", "acquire", "main"])["error"]["code"],
        "ACRE_INVALID_ARGUMENT"
    );
    assert_eq!(
        f.json(&["--json", "main", "--", "never-run"])["error"]["code"],
        "ACRE_JSON_CHILD_COMMAND"
    );
    assert_eq!(
        f.json(&["--json", "main", "system", "inspect"])["error"]["code"],
        "ACRE_INVALID_ARGUMENT"
    );
}

#[test]
fn fresh_copies_report_their_actual_source_and_strategy() {
    let f = Fixture::new();
    fs::create_dir(f.repo.join("node_modules")).unwrap();
    fs::write(f.repo.join("node_modules/marker"), "prepared").unwrap();
    let result = f.json(&["--json", "new", "work", "--stay"]);
    assert_eq!(result["reused"], false, "{result}");
    let environment = &result["environment"];
    match environment["cloneMode"].as_str() {
        // A copy is counted; a clone costs nothing to count.
        Some("copy") => {
            assert_eq!(environment["clonedFiles"], 1, "{result}");
            assert_eq!(environment["clonedBytes"], "prepared".len(), "{result}");
        }
        Some("reflink") => {}
        other => panic!("unexpected clone mode {other:?}: {result}"),
    }
    // The fixture keeps the repository and Acre's root on one volume, so on macOS the mode
    // follows that volume's filesystem rather than whatever cp happened to do.
    if cfg!(target_os = "macos") {
        let apfs = std::process::Command::new("/bin/df")
            .args(["-T", "apfs"])
            .arg(f.temp.path())
            .status()
            .unwrap()
            .success();
        assert_eq!(
            environment["cloneMode"],
            if apfs { "reflink" } else { "copy" },
            "{result}"
        );
    }
    assert_eq!(
        fs::canonicalize(result["environment"]["source"].as_str().unwrap()).unwrap(),
        fs::canonicalize(&f.repo).unwrap()
    );
}

#[test]
fn nested_projects_receive_their_environment() {
    let f = Fixture::new();
    fs::remove_file(f.repo.join("package.json")).unwrap();
    fs::create_dir_all(f.repo.join("apps/web")).unwrap();
    fs::write(f.repo.join("apps/web/package.json"), "{\"name\":\"nested\"}").unwrap();
    git(&f.repo, &["add", "-A"]);
    git(&f.repo, &["commit", "-qm", "nested project"]);
    fs::create_dir_all(f.repo.join("apps/web/node_modules")).unwrap();
    fs::write(f.repo.join("apps/web/node_modules/marker"), "prepared").unwrap();
    let path = f.open_workspace("work");
    assert_eq!(
        fs::read_to_string(path.join("apps/web/node_modules/marker")).unwrap(),
        "prepared"
    );
}

#[test]
fn plain_human_output_is_ascii() {
    let f = Fixture::new();
    let output = f.acre(&["--plain", "new", "work", "--stay"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_ascii());
}

#[test]
fn missing_legacy_metadata_does_not_hide_registered_workspaces() {
    let f = Fixture::new();
    let legacy = f.root.join("repositories/old-name-12345678");
    let path = legacy.join("workspaces/slot");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &f.repo,
        &["worktree", "add", "--detach", path.to_str().unwrap(), "HEAD"],
    );
    let inspected = f.json(&["--json", "system", "inspect"]);
    let recovered = inspected["state"]["workspaces"].as_array().unwrap();
    assert_eq!(recovered.len(), 1, "{inspected}");
    assert_eq!(recovered[0]["status"], "retained");
    assert_eq!(
        fs::canonicalize(recovered[0]["path"].as_str().unwrap()).unwrap(),
        fs::canonicalize(path).unwrap()
    );
}

#[test]
fn fresh_can_fetch_a_new_branch_from_an_explicit_remote() {
    let f = Fixture::new();
    git(&f.repo, &["remote", "add", "origin", f.repo.to_str().unwrap()]);
    git(
        &f.repo,
        &["remote", "add", "team/upstream", f.repo.to_str().unwrap()],
    );
    git(&f.repo, &["branch", "not-fetched"]);
    let result = f.json(&[
        "--json",
        "new",
        "work",
        "--from",
        "team/upstream/not-fetched",
        "--fresh",
        "--stay",
    ]);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(
        acre::git::refs::resolve_oid(&f.repo, "work").unwrap(),
        acre::git::refs::resolve_oid(&f.repo, "not-fetched").unwrap()
    );
}

/// Writes a `gh` stub that answers `gh pr view <n>` for any number with `head` as its commit.
#[cfg(unix)]
fn fake_gh(f: &Fixture, head: &str) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;
    let bin = f.temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = format!(
        "#!/bin/sh\ncat <<JSON\n{{\"number\": $3, \"title\": \"PR $3\", \"url\": \"https://github.com/example/shared/pull/$3\", \"baseRefName\": \"main\", \"headRefName\": \"f$3\", \"headRefOid\": \"{head}\", \"isCrossRepository\": false}}\nJSON\n"
    );
    fs::write(bin.join("gh"), script).unwrap();
    fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap()
}

#[cfg(unix)]
#[test]
fn done_with_a_pull_request_selector_never_returns_another_pull_request() {
    let f = Fixture::new();
    git(
        &f.repo,
        &["remote", "add", "origin", "https://github.com/example/shared.git"],
    );
    let inspected = f.json(&["system", "inspect", "--json"]);
    let path = fake_gh(
        &f,
        inspected["repository"]["currentWorktree"]["head"]
            .as_str()
            .unwrap(),
    );
    let opened: Value = serde_json::from_slice(
        &f.acre(&["pr:1", "--json"])
            .env("PATH", &path)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(opened["ok"], true, "{opened}");

    let output = f
        .acre(&["done", "pr:2", "--json"])
        .env("PATH", &path)
        .output()
        .unwrap();
    let done: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(done["error"]["code"], "ACRE_WORKSPACE_NOT_ACTIVE", "{done}");
    assert!(PathBuf::from(opened["path"].as_str().unwrap()).exists());
    let state = f.json(&["system", "inspect", "--json"]);
    assert!(state.to_string().contains("PR #1"), "{state}");
}

#[test]
fn git_environment_from_a_hook_does_not_redirect_acre() {
    let f = Fixture::new();
    let work = f.open_workspace("work");
    fs::write(work.join("file.txt"), "work").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "work in progress"]);
    let committed = acre::git::refs::resolve_oid(&work, "HEAD").unwrap().unwrap();
    // What Git exports to hooks and `rebase --exec` inside a linked worktree.
    let git_dir = acre::git::runner::run_git(&work, &["rev-parse", "--absolute-git-dir"]).unwrap();
    let git_dir = String::from_utf8(git_dir.stdout).unwrap();
    let created = f
        .acre(&["--json", "new", "other", "--stay"])
        .env("GIT_DIR", git_dir.trim())
        .env("GIT_WORK_TREE", &work)
        .output()
        .unwrap();
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(
        acre::git::refs::resolve_oid(&work, "refs/heads/work")
            .unwrap()
            .unwrap(),
        committed
    );
    assert!(branch_exists(&f.repo, "other"));
}

#[test]
fn unreadable_state_is_kept_until_repair_moves_it_aside() {
    let f = Fixture::new();
    f.open_workspace("work");
    let state = fs::read_dir(f.root.join("repositories"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path().join("state.json"))
        .find(|path| path.exists())
        .unwrap();
    fs::write(&state, "{ not json").unwrap();

    let refused = f.json(&["--json", "new", "other", "--stay"]);
    assert_eq!(refused["error"]["code"], "ACRE_STATE_UNREADABLE", "{refused}");
    assert_eq!(fs::read_to_string(&state).unwrap(), "{ not json");

    let repaired = f.json(&["--json", "system", "repair"]);
    assert_eq!(repaired["ok"], true, "{repaired}");
    let moved = PathBuf::from(repaired["report"]["quarantinedState"].as_str().unwrap());
    assert_eq!(fs::read_to_string(moved).unwrap(), "{ not json");
    let created = f.json(&["--json", "new", "other", "--stay"]);
    assert_eq!(created["ok"], true, "{created}");
}

#[test]
fn branch_names_git_would_expand_are_rejected_before_any_worktree_exists() {
    let f = Fixture::new();
    git(&f.repo, &["switch", "-qc", "elsewhere"]);
    git(&f.repo, &["switch", "-q", "main"]);
    let before = acre::git::worktrees::list_worktrees(&f.repo).unwrap().len();
    let refused = f.json(&["--json", "new", "@{-1}", "--stay"]);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        acre::git::worktrees::list_worktrees(&f.repo).unwrap().len(),
        before
    );
}

#[test]
fn a_branch_held_by_a_deleted_worktree_explains_how_to_recover() {
    let f = Fixture::new();
    git(&f.repo, &["branch", "feature"]);
    let external = f.temp.path().join("external");
    git(
        &f.repo,
        &["worktree", "add", "-q", external.to_str().unwrap(), "feature"],
    );
    fs::remove_dir_all(&external).unwrap();
    let opened = f.json(&["--json", "feature"]);
    assert_eq!(opened["error"]["code"], "ACRE_WORKTREE_MISSING", "{opened}");
}

#[test]
fn done_on_an_idle_slot_reports_it_as_pooled_not_external() {
    let f = Fixture::new();
    let warmed = f.json(&["--json", "system", "warm", "--slots", "1"]);
    assert_eq!(warmed["ok"], true, "{warmed}");
    let inspected = f.json(&["--json", "system", "inspect"]);
    let slot = inspected["state"]["slots"][0]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let done = f.json(&["--json", "done", &format!("worktree:{slot}")]);
    assert_eq!(done["pooled"], true, "{done}");
    assert_eq!(done["external"], false, "{done}");
}

#[cfg(unix)]
#[test]
fn release_keeps_a_workspace_git_cannot_fully_read_and_still_succeeds() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let acquired = f.json(&["--json", "acquire", "work", "--new", "--holder", "test"]);
    assert_eq!(acquired["ok"], true, "{acquired}");
    let path = PathBuf::from(acquired["path"].as_str().unwrap());
    // Ignored data Git cannot list: the return must not treat the gap as "nothing there".
    let locked = path.join("node_modules/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("file"), "data").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let released = f.json(&[
        "--json",
        "release",
        "--lease-id",
        acquired["lease_id"].as_str().unwrap(),
    ]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(released["ok"], true, "{released}");
    assert_eq!(released["retained"], true, "{released}");
    assert_eq!(
        released["return_error"]["code"], "ACRE_GIT_INCOMPLETE",
        "{released}"
    );
    assert!(locked.join("file").exists());
}

#[test]
fn bare_repository_layouts_release_leases_and_receive_seed_files() {
    let f = Fixture::new();
    fs::write(f.repo.join(".gitignore"), "node_modules\n.env\n").unwrap();
    git(&f.repo, &["commit", "-qam", "ignore env"]);
    let bare = f.temp.path().join("bare.git");
    git(
        f.temp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            f.repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let checkout = f.temp.path().join("checkout");
    git(
        &bare,
        &["worktree", "add", "-q", checkout.to_str().unwrap(), "main"],
    );
    fs::write(checkout.join(".env"), "secret").unwrap();

    let acquired = f.json(&[
        "--json",
        "-C",
        checkout.to_str().unwrap(),
        "acquire",
        "work",
        "--new",
        "--holder",
        "test",
    ]);
    assert_eq!(acquired["ok"], true, "{acquired}");
    let path = PathBuf::from(acquired["path"].as_str().unwrap());
    assert_eq!(fs::read_to_string(path.join(".env")).unwrap(), "secret");
    let released = f.json(&[
        "--json",
        "release",
        "--lease-id",
        acquired["lease_id"].as_str().unwrap(),
    ]);
    assert_eq!(released["ok"], true, "{released}");
}
