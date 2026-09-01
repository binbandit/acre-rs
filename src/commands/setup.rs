use std::fs;
use std::path::PathBuf;

use crate::error::{AcreError, Result, exit};
use crate::model::{CommandContext, SupportedShell};
use crate::state::config::{ensure_default_config_file, load_config};
use crate::state::paths::config_path;
use crate::ui::output::Renderer;
use crate::ui::prompt::confirm;
use crate::util::{ensure_directory, home_dir};

const START: &str = "# >>> acre >>>";
const END: &str = "# <<< acre <<<";

pub fn command_setup(context: &CommandContext, shell: Option<SupportedShell>, yes: bool) -> Result<i32> {
    let renderer = Renderer::new(context);
    let shell = shell.unwrap_or_else(detect_shell);
    let rc = shell_config_path(shell);
    let desired = format!("{START}\n{}\n{END}", shell_snippet(shell));
    let current = fs::read_to_string(&rc).unwrap_or_default();
    let next = install_block(&current, &desired);
    let changed = next != current;
    let approved = if changed {
        yes || !context.interactive
            || confirm(
                &renderer,
                &format!(
                    "{} Acre shell integration in {}?",
                    if current.contains(START) { "Update" } else { "Add" },
                    rc.display()
                ),
                true,
            )?
    } else {
        true
    };
    if approved && changed {
        if let Some(parent) = rc.parent() {
            ensure_directory(parent)?;
        }
        fs::write(&rc, next)
            .map_err(|error| AcreError::io(format!("could not write {}", rc.display()), error))?;
    }
    ensure_default_config_file()?;
    let config = load_config()?;
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": approved,
            "shell": shell,
            "shell_file": rc,
            "shell_changed": changed && approved,
            "acre_config": config_path(),
            "root": config.root,
        }));
    } else {
        renderer.line("<bold>Acre setup</bold>");
        renderer.line("");
        renderer.line(format!("  Shell       <blue>{:?}</blue>", shell));
        renderer.line(format!(
            "  Config      <dim>{}</dim>",
            renderer.value(config_path().display().to_string())
        ));
        renderer.line(format!(
            "  Workspaces  <dim>{}</dim>",
            renderer.value(config.root.display().to_string())
        ));
        renderer.line("");
        if !approved {
            renderer.line("<yellow>Shell integration was not changed.</yellow>");
        } else if !changed {
            renderer.line("<green>Shell integration is already current.</green>");
        } else {
            renderer.line(format!(
                "<green>{} shell integration in {}.</green>",
                if current.contains(START) {
                    "Updated"
                } else {
                    "Added"
                },
                renderer.value(rc.display().to_string())
            ));
        }
        renderer.line("Restart this shell, then open existing work with <blue>acre</blue> or start new work with <blue>acre new feature/name</blue>.");
    }
    Ok(if approved { exit::SUCCESS } else { exit::REFUSED })
}

pub fn detect_shell() -> SupportedShell {
    if cfg!(windows) || std::env::var_os("PSModulePath").is_some() {
        return SupportedShell::Powershell;
    }
    match std::env::var_os("SHELL")
        .and_then(|value| PathBuf::from(value).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(ToOwned::to_owned))
        .as_deref()
    {
        Some("fish") => SupportedShell::Fish,
        Some("bash") => SupportedShell::Bash,
        _ => SupportedShell::Zsh,
    }
}

fn shell_config_path(shell: SupportedShell) -> PathBuf {
    match shell {
        SupportedShell::Fish => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("fish/config.fish"),
        SupportedShell::Bash => home_dir().join(".bashrc"),
        SupportedShell::Powershell => {
            home_dir().join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
        }
        SupportedShell::Zsh => home_dir().join(".zshrc"),
    }
}

fn shell_snippet(shell: SupportedShell) -> &'static str {
    match shell {
        SupportedShell::Fish => "acre shell init fish | source",
        SupportedShell::Powershell => "Invoke-Expression (& acre shell init powershell | Out-String)",
        SupportedShell::Bash => "eval \"$(acre shell init bash)\"",
        SupportedShell::Zsh => "eval \"$(acre shell init zsh)\"",
    }
}

fn install_block(current: &str, desired: &str) -> String {
    if let Some(start) = current.find(START) {
        if let Some(relative_end) = current[start + START.len()..].find(END) {
            let end = start + START.len() + relative_end + END.len();
            return format!("{}{}{}", &current[..start], desired, &current[end..]);
        }
    }
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("{current}{separator}{desired}\n")
}
