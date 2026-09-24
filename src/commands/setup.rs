//! `acre setup`: install the shell integration block and create the configuration.

use std::fs;
use std::path::PathBuf;

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::shell::SupportedShell;
use crate::state::config::{ensure_default_config_file, load_config};
use crate::state::paths::config_path;
use crate::ui::output::Renderer;
use crate::ui::prompt::confirm;
use crate::util::{display_path, ensure_directory, home_dir};

const START: &str = "# >>> acre >>>";
const END: &str = "# <<< acre <<<";

/// One startup file and the contents setup wants it to have.
struct StartupFile {
    path: PathBuf,
    current: String,
    next: String,
}

impl StartupFile {
    fn read(path: PathBuf, snippet: &str) -> Result<Self> {
        let current = read_startup_file(&path)?;
        // Markers let a later run replace the block in place instead of appending a second copy.
        let next = install_block(&current, &format!("{START}\n{snippet}\n{END}"));
        Ok(Self { path, current, next })
    }

    fn changed(&self) -> bool {
        self.next != self.current
    }
}

pub fn run(context: &CommandContext, shell: Option<SupportedShell>, yes: bool) -> Result<i32> {
    let renderer = Renderer::new(context);
    let shell = shell.unwrap_or_else(detect_shell);
    let rc = shell_config_path(shell);
    // Read every destination before changing any file.
    let mut files = vec![StartupFile::read(rc.clone(), shell_snippet(shell))?];
    let login = (shell == SupportedShell::Bash).then(bash_login_path);
    if let Some(path) = &login {
        files.push(StartupFile::read(path.clone(), BASH_LOGIN_SNIPPET)?);
    }
    let changed: Vec<&StartupFile> = files.iter().filter(|file| file.changed()).collect();
    let names = changed
        .iter()
        .map(|file| display_path(&file.path))
        .collect::<Vec<_>>()
        .join(" and ");
    // Update only when every changed file already has a block; anything new is an addition.
    let verb = |past: bool| match (changed.iter().all(|file| file.current.contains(START)), past) {
        (true, false) => "Update",
        (true, true) => "Updated",
        (false, false) => "Add",
        (false, true) => "Added",
    };
    let approved = changed.is_empty()
        // Nobody to ask when piped; proceed rather than hang.
        || yes
        || !context.interactive
        || confirm(
            &renderer,
            &format!("{} Acre shell integration in {names}?", verb(false)),
            true,
        )?;
    if approved {
        for file in &changed {
            if let Some(parent) = file.path.parent() {
                ensure_directory(parent)?;
            }
            fs::write(&file.path, &file.next)
                .map_err(|error| AcreError::io(format!("could not write {}", file.path.display()), error))?;
        }
    }
    ensure_default_config_file()?;
    let config = load_config()?;
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": approved,
            "shell": shell,
            "shell_file": rc,
            "shell_changed": !changed.is_empty() && approved,
            "login_shell_file": login,
            "acre_config": config_path(),
            "root": config.root,
        }));
    } else {
        renderer.line("<bold>Acre setup</bold>");
        renderer.line("");
        renderer.line(format!("  Shell       <blue>{}</blue>", shell));
        renderer.line(format!(
            "  Config      <dim>{}</dim>",
            renderer.value(display_path(&config_path()))
        ));
        renderer.line(format!(
            "  Workspaces  <dim>{}</dim>",
            renderer.value(display_path(&config.root))
        ));
        renderer.line("");
        if !approved {
            renderer.line("<yellow>Shell integration was not changed.</yellow>");
        } else if changed.is_empty() {
            renderer.line("<green>Shell integration is already current.</green>");
        } else {
            renderer.line(format!(
                "<green>{} shell integration in {}.</green>",
                verb(true),
                renderer.value(&names)
            ));
        }
        renderer.line("Restart this shell, then open existing work with <blue>acre</blue> or start new work with <blue>acre new feature/name</blue>.");
    }
    Ok(if approved { exit::SUCCESS } else { exit::REFUSED })
}

pub fn detect_shell() -> SupportedShell {
    // PowerShell sets PSModulePath on every platform.
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
        // macOS default; the likeliest guess when SHELL is unset or unknown.
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
        // pwsh's $PROFILE.CurrentUserCurrentHost: Documents on Windows (wherever it is redirected),
        // the XDG config directory everywhere else.
        SupportedShell::Powershell => {
            let base = if cfg!(windows) {
                dirs::document_dir()
                    .unwrap_or_else(|| home_dir().join("Documents"))
                    .join("PowerShell")
            } else {
                std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home_dir().join(".config"))
                    .join("powershell")
            };
            base.join("Microsoft.PowerShell_profile.ps1")
        }
        SupportedShell::Zsh => std::env::var_os("ZDOTDIR")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join(".zshrc"),
    }
}

fn read_startup_file(path: &std::path::Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(AcreError::io(format!("could not read {}", path.display()), error)),
    }
}

fn bash_login_path() -> PathBuf {
    [".bash_profile", ".bash_login", ".profile"]
        .iter()
        .map(|name| home_dir().join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| home_dir().join(".bash_profile"))
}

// Login files such as .profile are also read by dash and by bash as sh (POSIX mode), neither of
// which can parse the bash integration.
const BASH_LOGIN_SNIPPET: &str =
    "if [ -n \"${BASH_VERSION:-}\" ] && ! shopt -oq posix; then eval \"$(acre shell init bash)\"; fi";

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
    // Don't glue our block onto a file that lacks a final newline.
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("{current}{separator}{desired}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_block_appends_once_and_replaces_in_place() {
        let block = format!("{START}\nnew\n{END}");
        assert_eq!(install_block("", &block), format!("{block}\n"));
        assert_eq!(
            install_block("export A=1", &block),
            format!("export A=1\n{block}\n")
        );
        let existing = format!("before\n{START}\nold\n{END}\nafter\n");
        assert_eq!(
            install_block(&existing, &block),
            format!("before\n{block}\nafter\n")
        );
    }
}
