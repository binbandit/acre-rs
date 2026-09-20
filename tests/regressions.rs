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
fn done_preserves_the_exact_previous_subdirectory() {
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
    let token = std::str::from_utf8(fields[3]).unwrap();
    let (mut command, _) = f.acre_in_shell(&["__resume", token], "subdirectory", &destination);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let (mut command, directive) = f.acre_in_shell(&["-"], "subdirectory", &destination);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let bytes = fs::read(directive).unwrap();
    let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    assert_eq!(
        fs::canonicalize(std::str::from_utf8(fields[2]).unwrap()).unwrap(),
        fs::canonicalize(from).unwrap()
    );
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
    assert!(matches!(
        result["environment"]["cloneMode"].as_str(),
        Some("copy" | "reflink")
    ));
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
