//! Workspace lifecycle and shell directive round trips against the real binary.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::prelude::*;
use common::{Fixture, branch_exists};

#[test]
fn python_environments_are_reused_at_their_installation_path() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.repo.join("package.json")).unwrap();
    fs::write(
        fixture.repo.join("pyproject.toml"),
        "[project]\nname = 'fixture'\n",
    )
    .unwrap();
    fs::write(fixture.repo.join(".gitignore"), ".venv\n.pytest_cache\n").unwrap();
    common::git(&fixture.repo, &["add", "-A"]);
    common::git(&fixture.repo, &["commit", "-qm", "python fixture"]);
    fs::create_dir(fixture.repo.join(".venv")).unwrap();
    fs::write(fixture.repo.join(".venv/pyvenv.cfg"), "home = /python\n").unwrap();

    let first = fixture.open_workspace("first");
    assert!(
        !first.join(".venv").exists(),
        "an external venv cannot be relocated"
    );
    fs::create_dir(first.join(".venv")).unwrap();
    fs::write(first.join(".venv/pyvenv.cfg"), "home = /python\n").unwrap();
    fs::write(first.join(".venv/installed-at"), first.to_str().unwrap()).unwrap();

    let done = fixture.json(&["done", "first", "--json"]);
    assert_eq!(done["pooled"], true, "{done}");
    assert!(first.join(".venv/installed-at").exists());
    let second = fixture.json(&["new", "second", "--stay", "--json"]);
    assert_eq!(second["path"], first.to_str().unwrap());
    assert_eq!(second["environment"]["state"], "ready", "{second}");

    let concurrent = fixture.open_workspace("concurrent");
    assert_ne!(concurrent, first);
    assert!(!concurrent.join(".venv").exists());
    assert_eq!(
        fs::read_to_string(first.join(".venv/installed-at")).unwrap(),
        first.to_str().unwrap()
    );
}

#[test]
fn repair_counts_only_records_it_actually_recovered_or_dropped() {
    let fixture = Fixture::new();
    let workspace = fixture.open_workspace("feature/repair");
    let warmed = fixture.json(&["system", "warm", "--json"]);
    assert_eq!(warmed["created"], 1, "{warmed}");

    fixture.forget_state();
    let report = fixture.json(&["system", "repair", "--json"]);
    assert_eq!(report["report"]["addedSlots"], 0, "{report}");
    assert_eq!(report["report"]["addedWorkspaces"], 2, "{report}");
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
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert!(inspected["state"]["leases"].as_array().unwrap().is_empty());

    let (mut forward, _) = fixture.acre_in_shell(&["-"], "shell-a", &fixture.repo);
    forward.assert().success();
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    let leases = inspected["state"]["leases"].as_array().unwrap();
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0]["sessionId"], "shell-a");
    assert_eq!(
        leases[0]["workspaceId"],
        inspected["state"]["workspaces"][0]["id"]
    );
}

#[test]
fn previous_location_does_not_wait_for_workspace_preparation() {
    use acre::git::repository::discover_repository;
    use acre::state::lock::RepositoryLock;
    use acre::state::paths::{repository_lock_path, repository_state_path};
    use std::time::Duration;

    let fixture = Fixture::new();
    let (mut new, directive) = fixture.acre_in_shell(&["new", "feature/nav"], "busy-shell", &fixture.repo);
    new.assert().success();
    let workspace = PathBuf::from(&directive_fields(&directive)[2]);
    let config = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    let repository = discover_repository(&fixture.repo).unwrap();
    let state_path = repository_state_path(&config, &repository);
    let before = fs::read(&state_path).unwrap();
    // A background replenisher may hold this lock while cloning a large environment.
    let _lock = RepositoryLock::acquire(&repository_lock_path(&config, &repository)).unwrap();
    for (from, to) in [(&workspace, &fixture.repo), (&fixture.repo, &workspace)] {
        let (navigate, directive) = fixture.acre_in_shell(&["-"], "busy-shell", from);
        assert_cmd::Command::from(navigate)
            .timeout(Duration::from_secs(3))
            .assert()
            .success();
        assert_eq!(
            fs::canonicalize(&directive_fields(&directive)[2]).unwrap(),
            fs::canonicalize(to).unwrap()
        );
    }
    assert_eq!(
        fs::read(state_path).unwrap(),
        before,
        "busy state must not be overwritten"
    );
}

#[test]
fn previous_location_moves_leases_between_repositories() {
    let fixture = Fixture::new();
    let other = Fixture::new();
    let (mut first, directive) = fixture.acre_in_shell(&["new", "first"], "cross-repo", &fixture.repo);
    first.assert().success();
    let first = PathBuf::from(&directive_fields(&directive)[2]);
    // Keep one Acre configuration and shell history while visiting another repository.
    let (mut second, directive) = fixture.acre_in_shell(&["new", "second"], "cross-repo", &other.repo);
    second.assert().success();
    let second = PathBuf::from(&directive_fields(&directive)[2]);
    for (from, to, source_repo, destination_repo) in [
        (&second, &first, &other.repo, &fixture.repo),
        (&first, &second, &fixture.repo, &other.repo),
    ] {
        let (mut navigate, directive) = fixture.acre_in_shell(&["-"], "cross-repo", from);
        navigate.assert().success();
        assert_eq!(PathBuf::from(&directive_fields(&directive)[2]), *to);
        for (repo, expected) in [(source_repo, 0), (destination_repo, 1)] {
            let output = fixture
                .acre(&["system", "inspect", "--json"])
                .current_dir(repo)
                .output()
                .unwrap();
            let inspected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(inspected["state"]["leases"].as_array().unwrap().len(), expected);
        }
    }
}

#[test]
fn new_resolves_exact_refs_and_preserves_base_selection() {
    use acre::git::refs::resolve_oid;
    let fixture = Fixture::new();
    let original = resolve_oid(&fixture.repo, "HEAD").unwrap().unwrap();
    common::git(
        &fixture.repo,
        &["remote", "add", "origin", fixture.repo.to_str().unwrap()],
    );
    common::git(
        &fixture.repo,
        &["update-ref", "refs/remotes/origin/main", &original],
    );
    fs::write(fixture.repo.join("new.txt"), "new commit").unwrap();
    common::git(&fixture.repo, &["add", "."]);
    common::git(&fixture.repo, &["commit", "-qm", "advance local main"]);
    let latest = resolve_oid(&fixture.repo, "HEAD").unwrap().unwrap();
    for (branch, flags, expected) in [
        ("remote-base", vec![], &original),
        ("explicit-base", vec!["--from", "main"], &latest),
        ("current-base", vec!["--from", "."], &latest),
        ("fresh-base", vec!["--fresh"], &latest),
    ] {
        let mut args = vec!["new", branch, "--stay", "--json"];
        args.extend(flags);
        let result = fixture.json(&args);
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(
            resolve_oid(&fixture.repo, &format!("refs/heads/{branch}"))
                .unwrap()
                .as_ref(),
            Some(expected)
        );
    }
    common::git(&fixture.repo, &["update-ref", "-d", "refs/remotes/origin/main"]);
    // A similarly named tag must not impersonate a local or remote branch.
    common::git(&fixture.repo, &["tag", "refs/heads/tag-shadow"]);
    common::git(&fixture.repo, &["tag", "refs/remotes/origin/main"]);
    let result = fixture.json(&["new", "tag-shadow", "--stay", "--json"]);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["base_ref"], "main");
    common::git(&fixture.repo, &["branch", "existing"]);
    fixture
        .acre(&["new", "existing", "--stay", "--json"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("ACRE_BRANCH_EXISTS"));
}

#[test]
fn new_checks_preferred_slots_until_one_is_safe() {
    use acre::git::repository::discover_repository;
    use acre::state::repository::{load_repository_state, save_repository_state};

    let fixture = Fixture::new();
    fixture
        .acre(&["config", "set", "pool.maxSlots", "3"])
        .assert()
        .success();
    fixture
        .acre(&["system", "warm", "--slots", "3"])
        .assert()
        .success();
    let config = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    let repository = discover_repository(&fixture.repo).unwrap();
    let mut state = load_repository_state(&config, &repository).unwrap();
    for (index, slot) in state.slots.iter_mut().enumerate() {
        slot.last_used_at = format!("2026-01-0{}T00:00:00.000Z", index + 1);
    }
    save_repository_state(&config, &repository, &state).unwrap();
    let oldest = &state.slots[0].path;
    let selected = &state.slots[1].path;
    let newest = &state.slots[2].path;
    fs::write(newest.join("package.json"), "user changes").unwrap();
    let trace = fixture.temp.path().join("git-trace.json");
    let output = fixture
        .acre(&["new", "feature/preferred", "--stay", "--json"])
        .env("GIT_TRACE2_EVENT", &trace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        fs::canonicalize(result["path"].as_str().unwrap()).unwrap(),
        fs::canonicalize(selected).unwrap()
    );
    assert_eq!(
        fs::read_to_string(newest.join("package.json")).unwrap(),
        "user changes"
    );
    let events: Vec<serde_json::Value> = fs::read_to_string(trace)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect();
    assert!(
        !events.iter().any(|event| event["argv"]
            .as_array()
            .is_some_and(|args| args.iter().any(|arg| arg == "for-each-ref"))),
        "new must not enumerate every ref"
    );
    let checked: Vec<PathBuf> = events
        .into_iter()
        .filter_map(|event| event["worktree"].as_str().map(PathBuf::from))
        .map(|path| fs::canonicalize(path).unwrap())
        .collect();
    assert!(checked.contains(&fs::canonicalize(newest).unwrap()));
    assert!(checked.contains(&fs::canonicalize(selected).unwrap()));
    assert!(
        !checked.contains(&fs::canonicalize(oldest).unwrap()),
        "unused slots should not be scanned"
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

    let (mut wrong_session, _) = fixture.acre_in_shell(&["__resume", &token], "another-shell", &destination);
    wrong_session.assert().failure();
    assert!(workspace.exists());

    let (mut still_inside, _) = fixture.acre_in_shell(&["__resume", &token], "shell-b", &workspace);
    still_inside.assert().failure();
    assert!(workspace.exists());

    let (mut resume, _) = fixture.acre_in_shell(&["__resume", &token], "shell-b", &destination);
    resume.assert().success();
    assert!(
        workspace.exists(),
        "the returned workspace stays at its installation path"
    );
    assert!(branch_exists(&fixture.repo, "feature/inside"));
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert_eq!(inspected["state"]["slots"][0]["status"], "idle");
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
    assert!(path.exists(), "the returned workspace stays in place");
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    assert_eq!(inspected["state"]["slots"][0]["status"], "idle");
    assert_eq!(inspected["state"]["workspaces"], serde_json::json!([]));
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

#[test]
fn seed_files_are_copied_scrubbed_and_retained_when_edited() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join(".gitignore"), "node_modules\n.env\n").unwrap();
    common::git(&fixture.repo, &["commit", "-am", "ignore local secrets"]);
    fs::write(fixture.repo.join(".env"), "local secret").unwrap();
    let first = fixture.open_workspace("first");
    assert_eq!(fs::read_to_string(first.join(".env")).unwrap(), "local secret");
    assert_eq!(fixture.json(&["done", "first", "--json"])["pooled"], true);
    let inspected = fixture.json(&["system", "inspect", "--json"]);
    let slot = PathBuf::from(inspected["state"]["slots"][0]["path"].as_str().unwrap());
    assert!(!slot.join(".env").exists());
    let second = fixture.open_workspace("second");
    fs::write(second.join(".env"), "edited secret").unwrap();
    let done = fixture.json(&["done", "second", "--json"]);
    assert_eq!(done["retained"], true, "{done}");
    assert_eq!(
        done["assessment"]["changedSeedFiles"],
        serde_json::json!([".env"])
    );
    assert_eq!(fs::read_to_string(second.join(".env")).unwrap(), "edited secret");
}
