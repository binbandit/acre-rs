//! Runs Git and other tools with captured output, timeouts, and uniform error reporting.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use process_wrap::std::StdCommandWrap;
use wait_timeout::ChildExt;

use crate::error::{AcreError, Result, exit};

/// Everything captured from a finished process.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    /// Exit statuses treated as success; any other status becomes an error.
    pub accepted_statuses: &'static [i32],
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            stdin: None,
            timeout: None,
            accepted_statuses: &[0],
        }
    }
}

pub fn run_process(executable: &str, args: &[&str], options: RunOptions) -> Result<ProcessResult> {
    let started = Instant::now();
    let mut command = Command::new(executable);
    command
        .args(args)
        // Always piped: git must never block waiting on the user's terminal for input.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    // Two error paths below want the same argv attached; build it once before either can fire.
    let details = serde_json::json!({ "executable": executable, "args": args });

    let mut command = StdCommandWrap::from(command);
    #[cfg(unix)]
    command.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::std::JobObject);

    let mut child = command.spawn().map_err(|error| {
        AcreError::new(
            "ACRE_PROCESS_START_FAILED",
            format!("Could not start {executable}: {error}"),
            exit::ENVIRONMENT,
        )
        .with_details(&details)
    })?;

    let deadline = options.timeout.map(|timeout| started + timeout);
    let timeout_error = || {
        AcreError::new(
            "ACRE_PROCESS_TIMEOUT",
            format!("{executable} exceeded its process timeout."),
            exit::ENVIRONMENT,
        )
        .with_details(&details)
    };
    let stdin = match (options.stdin, child.stdin().take()) {
        (Some(input), Some(mut stdin)) => Some(io_worker(move || stdin.write_all(&input))),
        _ => None,
    };
    let stdout = read_pipe(child.stdout().take().expect("stdout configured as piped"));
    let stderr = read_pipe(child.stderr().take().expect("stderr configured as piped"));

    let captured = (|| {
        // Keep the parent unreaped until I/O finishes, so its process-group ID cannot be reused
        // before timeout cleanup. All streams and the final wait share one deadline.
        let stdout = receive_io(stdout, deadline).map_err(|()| timeout_error())??;
        let stderr = receive_io(stderr, deadline).map_err(|()| timeout_error())??;
        let input = stdin
            .map(|stdin| receive_io(stdin, deadline).map_err(|()| timeout_error()))
            .transpose()?;
        let status = match deadline {
            Some(deadline) => child
                .inner_mut()
                .wait_timeout(deadline.saturating_duration_since(Instant::now())),
            None => child.inner_mut().wait().map(Some),
        }
        .map_err(|error| AcreError::io(format!("could not wait for {executable}"), error))?
        .ok_or_else(timeout_error)?;
        Ok((status, stdout, stderr, input))
    })();
    let (status, stdout, stderr, input) = match captured {
        Ok(captured) => captured,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.inner_mut().kill();
            let _ = child.inner_mut().wait();
            return Err(error);
        }
    };

    // No code means a signal killed it; call that a plain failure rather than success.
    let status_code = status.code().unwrap_or(1);
    let result = ProcessResult {
        argv: std::iter::once(executable)
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect(),
        cwd: options.cwd,
        status: status_code,
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
    };

    // Callers that expect a non-zero answer (rev-parse --verify, check-ignore) opt in per call.
    if options.accepted_statuses.contains(&status_code) {
        if let Some(input) = input {
            input?;
        }
        Ok(result)
    } else {
        Err(process_failure(result))
    }
}

fn io_worker<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Receiver<std::io::Result<T>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(work());
    });
    receive
}

fn read_pipe(mut pipe: impl Read + Send + 'static) -> Receiver<std::io::Result<Vec<u8>>> {
    io_worker(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_| bytes)
    })
}

fn receive_io<T>(
    receive: Receiver<std::io::Result<T>>,
    deadline: Option<Instant>,
) -> std::result::Result<Result<T>, ()> {
    let received = match deadline {
        Some(deadline) => receive.recv_timeout(deadline.saturating_duration_since(Instant::now())),
        None => receive.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    match received {
        Ok(result) => {
            Ok(result.map_err(|error| AcreError::io("could not transfer child-process data", error)))
        }
        Err(RecvTimeoutError::Timeout) => Err(()),
        Err(RecvTimeoutError::Disconnected) => Ok(Err(AcreError::new(
            "ACRE_PROCESS_IO",
            "A child-process I/O worker failed.",
            exit::INTERNAL,
        ))),
    }
}

pub fn run_git(cwd: &Path, args: &[&str]) -> Result<ProcessResult> {
    run_git_with(cwd, args, RunOptions::default())
}

pub fn run_git_with(cwd: &Path, args: &[&str], options: RunOptions) -> Result<ProcessResult> {
    let args: Vec<&str> = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    run_process(
        "git",
        &args,
        RunOptions {
            cwd: Some(cwd.to_path_buf()),
            ..options
        },
    )
}

/// Runs a program with the user's terminal attached, for `acre <target> -- <command>`.
pub fn run_passthrough(executable: &str, args: &[std::ffi::OsString], cwd: &Path) -> Result<i32> {
    let status = Command::new(executable)
        .args(args)
        .current_dir(cwd)
        // The user's command owns the terminal: editors and REPLs need all three streams.
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| {
            AcreError::new(
                "ACRE_PROCESS_START_FAILED",
                format!("Could not start {executable}: {error}"),
                exit::ENVIRONMENT,
            )
        })?;
    Ok(status.code().unwrap_or(1))
}

pub fn decode_stdout(result: &ProcessResult) -> String {
    String::from_utf8_lossy(&result.stdout).trim().to_owned()
}

fn process_failure(result: ProcessResult) -> AcreError {
    let stderr = String::from_utf8_lossy(&result.stderr).trim().to_owned();
    // argv[0] tells us whether this was git, which gets its own error code and exit class.
    let executable = result
        .argv
        .first()
        .cloned()
        .unwrap_or_else(|| "process".to_owned());
    // Git's stderr is the user-facing message; fall back to the status only when it said nothing.
    let message = if stderr.is_empty() {
        format!("{executable} exited with status {}.", result.status)
    } else {
        stderr.clone()
    };
    AcreError::new(
        if executable == "git" {
            "ACRE_GIT_FAILED"
        } else {
            "ACRE_PROCESS_FAILED"
        },
        message,
        if executable == "git" {
            exit::GIT
        } else {
            exit::ENVIRONMENT
        },
    )
    .with_details(serde_json::json!({
        "argv": result.argv,
        "cwd": result.cwd,
        "status": result.status,
        "stderr": stderr,
    }))
}
