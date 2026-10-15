//! Cleanup through the real CLI, with Git and Acre state checked afterwards.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, branch_exists, git};

fn commit_file(path: &Path) {
    fs::write(path.join("feature.txt"), "finished work\n").unwrap();
    git(path, &["add", "feature.txt"]);
    git(path, &["commit", "-qm", "feature"]);
}

#[test]
fn clean_previews_then_returns_merged_workspaces_and_deletes_branches() {
    let f = Fixture::new();
    let workspace = f.open_workspace("finished");
    commit_file(&workspace);
    git(&f.repo, &["merge", "--ff-only", "finished"]);
    git(&f.repo, &["branch", "already-returned"]);

    let repository = acre::git::repository::discover_repository(&f.repo).unwrap();
    let state_path = f
        .root
        .join("repositories")
        .join(&repository.id)
        .join("state.json");
    let saved_state = fs::read(&state_path).unwrap();

    let preview = f.json(&["--json", "clean"]);
    assert_eq!(preview["ok"], true, "{preview}");
    assert_eq!(preview["report"]["dryRun"], true, "{preview}");
    assert_eq!(fs::read(&state_path).unwrap(), saved_state);
    assert!(branch_exists(&f.repo, "finished"));
    assert!(workspace.join(".git").exists());
    assert_eq!(
        f.json(&["--json", "system", "inspect"])["state"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let cleaned = f.json(&["--json", "clean", "--yes"]);
    assert_eq!(cleaned["ok"], true, "{cleaned}");
    assert!(!branch_exists(&f.repo, "finished"));
    assert!(!branch_exists(&f.repo, "already-returned"));
    assert!(branch_exists(&f.repo, "main"));
    let state = f.json(&["--json", "system", "inspect"]);
    assert!(state["state"]["workspaces"].as_array().unwrap().is_empty());
    assert_eq!(state["state"]["slots"][0]["status"], "idle", "{state}");
    let completions = f.json(&["--json", "__complete", "finished"]);
    assert!(completions["candidates"].as_array().unwrap().is_empty());
    assert_eq!(
        f.json(&["--json", "clean", "--yes"])["report"]["cleaned"],
        serde_json::json!([])
    );
}

#[test]
fn clean_keeps_unsafe_workspaces_and_their_branches() {
    for (case, reason) in [
        ("dirty", "tracked or untracked"),
        ("leased", "another client"),
        ("locked", "locked"),
        ("ignored", "ignored files"),
        ("unmerged", "not merged"),
        ("detached", "detached commits"),
    ] {
        let f = Fixture::new();
        let workspace = f.open_workspace(case);
        match case {
            "dirty" => fs::write(workspace.join("unfinished"), "keep me").unwrap(),
            "leased" => {
                assert_eq!(
                    f.json(&["--json", "acquire", case, "--holder", "editor"])["ok"],
                    true
                );
            }
            "locked" => git(&f.repo, &["worktree", "lock", workspace.to_str().unwrap()]),
            "ignored" => {
                fs::write(workspace.join(".gitignore"), "node_modules\nprivate\n").unwrap();
                git(&workspace, &["add", ".gitignore"]);
                git(&workspace, &["commit", "-qm", "ignore private data"]);
                git(&f.repo, &["merge", "--ff-only", case]);
                fs::write(workspace.join("private"), "keep me").unwrap();
            }
            "unmerged" => commit_file(&workspace),
            "detached" => {
                git(&workspace, &["switch", "--detach"]);
                commit_file(&workspace);
                // Even an ancestor of main needs a named branch before detaching again.
                let head = acre::git::refs::resolve_oid(&workspace, "HEAD").unwrap().unwrap();
                git(&f.repo, &["merge", "--ff-only", &head]);
            }
            _ => unreachable!(),
        }
        let output = f.json(&["--json", "clean", "--yes"]);
        assert_eq!(output["ok"], true, "{case}: {output}");
        assert!(
            output["report"]["skipped"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| { item["reason"].as_str().unwrap().contains(reason) }),
            "{case}: {output}"
        );
        assert!(workspace.join(".git").exists(), "{case}");
        assert!(branch_exists(&f.repo, case), "{case}");
    }
}

#[test]
fn clean_keeps_the_calling_workspace_even_with_directory_override() {
    let f = Fixture::new();
    let workspace = f.open_workspace("current");
    for directory in [&f.repo, &workspace] {
        let output = f
            .acre(&["-C", directory.to_str().unwrap(), "--json", "clean", "--yes"])
            .current_dir(&workspace)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(workspace.join(".git").exists());
        assert!(branch_exists(&f.repo, "current"));
    }
}

#[test]
fn clean_prunes_missing_owned_workspaces_but_preserves_external_and_locked_ones() {
    let f = Fixture::new();
    let missing = f.open_workspace("missing");
    let locked = f.open_workspace("locked");
    let external = f.temp.path().join("external");
    git(
        &f.repo,
        &["worktree", "add", "-b", "external", external.to_str().unwrap()],
    );
    git(&f.repo, &["worktree", "lock", locked.to_str().unwrap()]);
    for path in [&missing, &locked, &external] {
        fs::remove_dir_all(path).unwrap();
    }
    let preview = f.json(&["--json", "clean", "--dry-run"]);
    assert_eq!(preview["ok"], true, "{preview}");
    let before = f.json(&["--json", "system", "inspect"]);
    assert_eq!(before["repository"]["worktrees"].as_array().unwrap().len(), 4);
    let cleaned = f.json(&["--json", "clean", "--yes"]);
    assert_eq!(cleaned["ok"], true, "{cleaned}");
    let after = f.json(&["--json", "system", "inspect"]);
    assert_eq!(
        after["repository"]["worktrees"].as_array().unwrap().len(),
        3,
        "{after}"
    );
    assert_eq!(
        after["state"]["workspaces"].as_array().unwrap().len(),
        1,
        "{after}"
    );
    assert!(!branch_exists(&f.repo, "missing"));
    assert!(branch_exists(&f.repo, "external"));
    assert!(branch_exists(&f.repo, "locked"));
}

#[test]
fn clean_retains_squash_merges_and_supports_an_explicit_base() {
    let f = Fixture::new();
    let workspace = f.open_workspace("squashed");
    commit_file(&workspace);
    git(&f.repo, &["merge", "--squash", "squashed"]);
    git(&f.repo, &["commit", "-qm", "squash feature"]);
    assert_eq!(f.json(&["--json", "clean", "--yes"])["ok"], true);
    assert!(branch_exists(&f.repo, "squashed"));
    assert!(workspace.join(".git").exists());
    git(&f.repo, &["branch", "integrated", "squashed"]);
    let cleaned = f.json(&["--json", "clean", "--yes", "--base", "integrated"]);
    assert_eq!(cleaned["ok"], true, "{cleaned}");
    assert!(!branch_exists(&f.repo, "squashed"));
    assert!(branch_exists(&f.repo, "integrated"));
    assert!(branch_exists(&f.repo, "main"));
    assert_eq!(
        f.json(&["--json", "clean", "--yes", "--base", "absent"])["ok"],
        false
    );
}

#[test]
fn clean_prefers_the_remote_default_without_deleting_remote_refs() {
    let f = Fixture::new();
    git(&f.repo, &["remote", "add", "origin", "/unused-offline-remote"]);
    git(&f.repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(
        &f.repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let workspace = f.open_workspace("work");
    commit_file(&workspace);
    git(&f.repo, &["update-ref", "refs/remotes/origin/main", "work"]);
    let cleaned = f.json(&["--json", "clean", "--yes"]);
    assert_eq!(cleaned["ok"], true, "{cleaned}");
    assert_eq!(cleaned["report"]["base"], "origin/main");
    assert!(!branch_exists(&f.repo, "work"));
    git(&f.repo, &["show-ref", "--verify", "refs/remotes/origin/main"]);
}

#[test]
fn clean_preserves_branches_during_a_git_operation() {
    let f = Fixture::new();
    commit_file(&f.repo);
    let workspace = f.open_workspace("bisecting");
    git(
        &workspace,
        &["bisect", "start", "--no-checkout", "HEAD", "HEAD~1"],
    );
    git(&f.repo, &["branch", "merged"]);
    let report = f.json(&["--json", "clean", "--yes"]);
    assert_eq!(report["ok"], true, "{report}");
    assert!(branch_exists(&f.repo, "bisecting"));
    assert!(branch_exists(&f.repo, "merged"));
    assert!(workspace.join(".git").exists());
    assert!(
        report["report"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["branch"] == "merged" && item["reason"].as_str().unwrap().contains("operation")
            }),
        "{report}"
    );
}

#[test]
fn clean_drops_gone_records_without_losing_unmerged_commits() {
    let f = Fixture::new();
    let workspace = f.open_workspace("unmerged");
    commit_file(&workspace);
    git(&f.repo, &["worktree", "remove", workspace.to_str().unwrap()]);
    let report = f.json(&["--json", "clean", "--yes"]);
    assert_eq!(report["ok"], true, "{report}");
    assert!(branch_exists(&f.repo, "unmerged"));
    let state = f.json(&["--json", "system", "inspect"]);
    assert!(state["state"]["workspaces"].as_array().unwrap().is_empty());
    assert!(state["state"]["slots"].as_array().unwrap().is_empty());
}

#[test]
fn cleanup_ref_transaction_rejects_a_moved_branch_or_base() {
    use acre::git::operations::delete_merged_branch;
    use acre::git::refs::list_refs;
    use acre::git::repository::discover_repository;

    for moved in ["main", "merged"] {
        let f = Fixture::new();
        git(&f.repo, &["branch", "merged"]);
        let repository = discover_repository(&f.repo).unwrap();
        let refs = list_refs(&f.repo).unwrap();
        let branch = refs
            .iter()
            .find(|reference| reference.short_name == "merged")
            .unwrap();
        let base = refs
            .iter()
            .find(|reference| reference.short_name == "main")
            .unwrap();
        git(&f.repo, &["switch", moved]);
        commit_file(&f.repo);
        assert!(delete_merged_branch(&repository, branch, base).is_err());
        assert!(branch_exists(&f.repo, "merged"));
    }
}
