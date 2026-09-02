//! The directive file: how Acre tells the wrapping shell function to `cd` or to resume a command.

use std::fs;
use std::path::Path;

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};

// Checked by the generated shell functions; bump it and they refuse rather than misread.
pub const DIRECTIVE_VERSION: &str = "acre-directive-v1";

pub fn write_cd_directive(context: &CommandContext, target: &Path) -> Result<()> {
    write_directive(context, "cd", target, None)
}

pub fn write_resume_directive(context: &CommandContext, destination: &Path, token: &str) -> Result<()> {
    write_directive(context, "resume-after-cd", destination, Some(token))
}

fn write_directive(context: &CommandContext, action: &str, target: &Path, token: Option<&str>) -> Result<()> {
    let path = context.shell.directive_file.as_ref().ok_or_else(|| {
        AcreError::new(
            "ACRE_SHELL_INTEGRATION_REQUIRED",
            "Acre shell integration is not active.",
            exit::ENVIRONMENT,
        )
    })?;
    let mut bytes = Vec::new();
    for value in [
        DIRECTIVE_VERSION.as_bytes(),
        action.as_bytes(),
        target.to_string_lossy().as_bytes(),
        token.unwrap_or("").as_bytes(),
    ] {
        bytes.extend_from_slice(value);
        // NUL-separated so a path with a newline or space survives the round trip.
        bytes.push(0);
    }
    fs::write(path, bytes)
        .map_err(|error| AcreError::io(format!("could not write {}", path.display()), error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // It sits in a shared temp dir; nobody else needs to read where the shell is going.
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
