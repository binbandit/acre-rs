//! Exercises captured child processes with real pipes and bounded completion.

use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use acre::git::runner::{RunOptions, run_process};

#[test]
fn timeout_stops_descendants_after_the_parent_exits() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("spawn-descendant"), "").unwrap();
    let executable = std::env::current_exe().unwrap();
    let result = run_process(
        executable.to_str().unwrap(),
        &["--exact", "descendant_parent", "--nocapture"],
        RunOptions {
            cwd: Some(directory.path().to_path_buf()),
            timeout: Some(Duration::from_secs(1)),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.unwrap_err().code, "ACRE_PROCESS_TIMEOUT");
    assert!(directory.path().join("parent-started").exists());
    std::thread::sleep(Duration::from_secs(2));
    assert!(!directory.path().join("descendant-finished").exists());
}

#[test]
fn descendant_parent() {
    if !std::path::Path::new("spawn-descendant").exists() {
        return;
    }
    // Deliberately orphan a child that inherits the capture pipes.
    #[allow(clippy::zombie_processes)]
    let _child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "descendant_leaf", "--nocapture"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    fs::write("parent-started", "").unwrap();
}

#[test]
fn descendant_leaf() {
    if !std::path::Path::new("spawn-descendant").exists() {
        return;
    }
    std::thread::sleep(Duration::from_secs(2));
    fs::write("descendant-finished", "").unwrap();
}

#[cfg(unix)]
#[test]
fn timeout_still_applies_after_output_pipes_close() {
    let result = run_process(
        "sh",
        &["-c", "exec >/dev/null 2>&1; sleep 2"],
        RunOptions {
            timeout: Some(Duration::from_millis(200)),
            ..RunOptions::default()
        },
    );
    assert_eq!(result.unwrap_err().code, "ACRE_PROCESS_TIMEOUT");
}

#[cfg(unix)]
#[test]
fn timeout_includes_output_inherited_by_descendants() {
    use acre::git::runner::{RunOptions, run_process};
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let result = run_process(
        "sh",
        &["-c", "sleep 2 & exit 0"],
        RunOptions {
            timeout: Some(Duration::from_millis(200)),
            ..RunOptions::default()
        },
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(result.unwrap_err().code, "ACRE_PROCESS_TIMEOUT");
}

#[cfg(unix)]
#[test]
fn large_input_and_output_do_not_deadlock() {
    use acre::git::runner::{RunOptions, run_process};
    use std::sync::mpsc;
    use std::time::Duration;

    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_process(
            "sh",
            &["-c", "head -c 262144 /dev/zero; cat"],
            RunOptions {
                stdin: Some(vec![b'x'; 262144]),
                timeout: Some(Duration::from_secs(3)),
                ..RunOptions::default()
            },
        );
        let _ = send.send(result);
    });
    let result = receive
        .recv_timeout(Duration::from_secs(5))
        .expect("child process deadlocked")
        .expect("child output");
    assert_eq!(result.stdout.len(), 524288);
    assert!(result.stdout[262144..].iter().all(|byte| *byte == b'x'));
}
