use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::error::{AcreError, Result, exit};
use crate::model::ProcessResult;

#[derive(Debug, Clone)]
pub struct RunOptions<'a> {
    pub cwd: Option<&'a Path>,
    pub stdin: Option<&'a [u8]>,
    pub timeout: Option<Duration>,
    pub accepted_statuses: &'a [i32],
}

impl<'a> Default for RunOptions<'a> {
    fn default() -> Self {
        Self {
            cwd: None,
            stdin: None,
            timeout: None,
            accepted_statuses: &[0],
        }
    }
}

pub fn run_process(executable: &str, args: &[String], options: RunOptions<'_>) -> Result<ProcessResult> {
    let started = Instant::now();
    let mut command = Command::new(executable);
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = options.cwd {
        command.current_dir(cwd);
    }

    let mut child = command.spawn().map_err(|error| {
        AcreError::new(
            "ACRE_PROCESS_START_FAILED",
            format!("Could not start {executable}: {error}"),
            exit::ENVIRONMENT,
        )
        .with_details(serde_json::json!({ "executable": executable, "args": args }))
    })?;

    if let Some(input) = options.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input)
                .map_err(|error| AcreError::io(format!("could not write to {executable}"), error))?;
        }
    } else {
        drop(child.stdin.take());
    }

    let mut stdout = child.stdout.take().expect("stdout configured as piped");
    let mut stderr = child.stderr.take().expect("stderr configured as piped");
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let status = if let Some(timeout) = options.timeout {
        match child
            .wait_timeout(timeout)
            .map_err(|error| AcreError::io(format!("could not wait for {executable}"), error))?
        {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AcreError::new(
                    "ACRE_PROCESS_TIMEOUT",
                    format!("{executable} did not finish within {} seconds.", timeout.as_secs()),
                    exit::ENVIRONMENT,
                )
                .with_details(serde_json::json!({ "executable": executable, "args": args })));
            }
        }
    } else {
        child
            .wait()
            .map_err(|error| AcreError::io(format!("could not wait for {executable}"), error))?
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    let status_code = status.code().unwrap_or(1);
    let result = ProcessResult {
        argv: std::iter::once(executable.to_owned()).chain(args.iter().cloned()).collect(),
        cwd: options.cwd.map(Path::to_path_buf),
        status: status_code,
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
    };

    if options.accepted_statuses.contains(&status_code) {
        Ok(result)
    } else {
        Err(process_failure(result))
    }
}

pub fn run_git(cwd: &Path, args: &[&str]) -> Result<ProcessResult> {
    run_git_with(cwd, args.iter().map(|value| (*value).to_owned()).collect(), RunOptions::default())
}

pub fn run_git_strings(cwd: &Path, args: Vec<String>) -> Result<ProcessResult> {
    run_git_with(cwd, args, RunOptions::default())
}

pub fn run_git_with(cwd: &Path, args: Vec<String>, mut options: RunOptions<'_>) -> Result<ProcessResult> {
    options.cwd = Some(cwd);
    run_process("git", &args, options)
}

pub fn run_passthrough(executable: &str, args: &[std::ffi::OsString], cwd: &Path) -> Result<i32> {
    let status = Command::new(executable)
        .args(args)
        .current_dir(cwd)
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
    let executable = result.argv.first().cloned().unwrap_or_else(|| "process".to_owned());
    let message = if stderr.is_empty() {
        format!("{executable} exited with status {}.", result.status)
    } else {
        stderr.clone()
    };
    AcreError::new(
        if executable == "git" { "ACRE_GIT_FAILED" } else { "ACRE_PROCESS_FAILED" },
        message,
        if executable == "git" { exit::GIT } else { exit::ENVIRONMENT },
    )
    .with_details(serde_json::json!({
        "argv": result.argv,
        "cwd": result.cwd,
        "status": result.status,
        "stderr": stderr,
    }))
}
