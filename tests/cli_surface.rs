//! Regression coverage for the command-line surface: rendering, configuration, doctor, setup, and providers.

mod common;

use std::fs;

use common::Fixture;
use serde_json::json;

#[test]
fn user_text_prints_verbatim_in_plain_output() {
    let f = Fixture::new();
    for branch in ["feat<1>", "fix/café"] {
        let output = f.acre(&["--plain", "new", branch, "--stay"]).output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        // Printed names and commands must survive a copy and paste.
        assert!(stdout.contains(&format!("Created {branch}")), "{stdout}");
        assert!(!stdout.contains("\\u{"), "{stdout}");
    }
}

#[test]
fn mistyped_flags_are_usage_errors() {
    let f = Fixture::new();
    let output = f.acre(&["--json", "--bogus"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "ACRE_INVALID_ARGUMENT", "{error}");
}

#[test]
fn configuration_rejects_typos_and_keeps_the_schema_hint() {
    let f = Fixture::new();
    fs::write(&f.config, r#"{"pool": {"maxSlot": 3}}"#).unwrap();
    let output = f.acre(&["--json", "config", "show"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "ACRE_CONFIG_INVALID", "{error}");

    fs::write(
        &f.config,
        json!({ "$schema": "acre.schema.json", "root": f.root }).to_string(),
    )
    .unwrap();
    assert_eq!(
        f.json(&[
            "--json",
            "config",
            "set",
            "environment.seedFiles",
            r#"[".env", ".env"]"#
        ])["ok"],
        false
    );
    assert_eq!(
        f.json(&["--json", "config", "set", "root", "2024"])["value"],
        "2024"
    );
    let shown = f.json(&["--json", "config", "show"]);
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["config"]["$schema"], "acre.schema.json", "{shown}");
    assert_eq!(shown["config"]["root"], "2024", "{shown}");
}

#[test]
fn doctor_passes_with_an_empty_pool_and_reports_bad_configuration() {
    let f = Fixture::new();
    let output = f.acre(&["--json", "system", "doctor"]).output().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{report}");
    let pool = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "Warm pool")
        .unwrap();
    assert_eq!(pool["level"], "warn", "{report}");

    fs::write(&f.config, r#"{"pool": {"maxSlots": 0}}"#).unwrap();
    let output = f.acre(&["--json", "system", "doctor"]).output().unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(3), "{report}");
    assert_eq!(report["checks"][1]["name"], "Configuration", "{report}");
    assert_eq!(report["checks"][1]["level"], "fail", "{report}");
}

#[cfg(unix)]
#[test]
fn bash_login_profiles_are_safe_for_posix_shells() {
    let f = Fixture::new();
    let home = f.temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let home = fs::canonicalize(home).unwrap();
    fs::write(home.join(".profile"), "export BEFORE=1\n").unwrap();
    let output = f
        .acre(&["setup", "--shell", "bash", "--yes"])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("~/.bashrc and ~/.profile"), "{stdout}");
    fs::write(
        home.join(".profile"),
        fs::read_to_string(home.join(".profile")).unwrap() + "export AFTER=1\n",
    )
    .unwrap();
    // sh (dash, or bash in POSIX mode) must read the whole file without choking on bash syntax.
    let sourced = std::process::Command::new("sh")
        .args(["-c", ". \"$HOME/.profile\" && printf %s \"$AFTER\""])
        .env("HOME", &home)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&sourced.stdout), "1");
    assert!(
        sourced.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&sourced.stderr)
    );
}

#[cfg(unix)]
#[test]
fn pull_request_provider_failures_use_their_exit_classes() {
    use std::os::unix::fs::PermissionsExt;

    let f = Fixture::new();
    common::git(
        &f.repo,
        &["remote", "add", "origin", "https://github.com/example/shared.git"],
    );
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let path = std::iter::once(bin.clone())
        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))
        .collect::<Vec<_>>();
    let path = std::env::join_paths(path).unwrap();
    for (stderr, exit, code) in [
        (
            "error connecting to api.github.com\ncheck your internet connection",
            8,
            "ACRE_PR_LOOKUP_FAILED",
        ),
        (
            "GraphQL: Could not resolve to a PullRequest with the number of 7.",
            4,
            "ACRE_TARGET_NOT_FOUND",
        ),
    ] {
        fs::write(
            bin.join("gh"),
            format!("#!/bin/sh\nprintf '%s\\n' '{stderr}' >&2\nexit 1\n"),
        )
        .unwrap();
        fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
        let output = f.acre(&["--json", "pr:7"]).env("PATH", &path).output().unwrap();
        let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(exit), "{error}");
        assert_eq!(error["error"]["code"], code, "{error}");

        // Human output keeps the provider's lines as lines, not escaped newlines.
        let output = f.acre(&["pr:7"]).env("PATH", &path).output().unwrap();
        let rendered = String::from_utf8(output.stderr).unwrap();
        assert!(!rendered.contains("\\x0a"), "{rendered}");
        assert!(!rendered.contains("acre new"), "{rendered}");
    }
}
