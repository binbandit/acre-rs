//! `acre config`: show, edit, and set user configuration.

use std::process::{Command, Stdio};

use crate::cli::CommandContext;
use crate::error::{AcreError, Result, exit};
use crate::git::repository::discover_repository;
use crate::model::AcreConfig;
use crate::state::config::{load_config, save_config, set_config_value, write_default_repo_config};
use crate::state::paths::config_path;
use crate::ui::output::Renderer;

pub fn show(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "config": config }));
    } else {
        renderer.raw(format!("{}\n", serde_json::to_string_pretty(&config)?));
    }
    Ok(exit::SUCCESS)
}

pub fn path(context: &CommandContext) -> Result<i32> {
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "path": config_path()}));
    } else {
        renderer.raw(format!("{}\n", config_path().display()));
    }
    Ok(exit::SUCCESS)
}

pub fn init(context: &CommandContext, force: bool) -> Result<i32> {
    let target = config_path();
    if !force && target.exists() {
        return Err(AcreError::new(
            "ACRE_CONFIG_EXISTS",
            "Acre configuration already exists.",
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "path": target })));
    }
    save_config(&AcreConfig::default())?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "path": target}));
    } else {
        renderer.line(format!("<green>Created</green> <dim>{}</dim>", target.display()));
    }
    Ok(exit::SUCCESS)
}

pub fn set(context: &CommandContext, key: &str, value: &str) -> Result<i32> {
    let mut config = load_config()?;
    let parsed = set_config_value(&mut config, key, value)?;
    save_config(&config)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "key": key, "value": parsed}));
    } else {
        renderer.line(format!(
            "<green>Set</green> <blue>{}</blue> = {}",
            renderer.value(key),
            renderer.value(parsed.to_string())
        ));
    }
    Ok(exit::SUCCESS)
}

pub fn edit(context: &CommandContext) -> Result<i32> {
    if context.global.json {
        return Err(AcreError::new(
            "ACRE_INTERACTIVE_COMMAND",
            "Use config set to edit configuration in JSON mode.",
            exit::USAGE,
        ));
    }
    let target = config_path();
    // Create the defaults first so the editor opens a real file rather than an empty buffer.
    if !target.exists() {
        save_config(&AcreConfig::default())?;
    }
    // Like git: an empty $VISUAL falls through to $EDITOR.
    let editor = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AcreError::new(
                "ACRE_EDITOR_NOT_CONFIGURED",
                "Set $VISUAL or $EDITOR before using acre config edit.",
                exit::ENVIRONMENT,
            )
        })?;
    let status = editor_command(&editor, &target)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| AcreError::io("could not start editor", error))?;
    Ok(status.code().unwrap_or(exit::INTERNAL))
}

/// Runs the editor the way git does: through the shell, so quoting and flags ("code --wait")
/// mean what they mean at a prompt, with the file passed as a separate argument.
fn editor_command(editor: &str, file: &std::path::Path) -> Command {
    let mut command;
    #[cfg(unix)]
    {
        command = Command::new("sh");
        command.arg("-c").arg(format!("{editor} \"$@\"")).arg(editor);
    }
    #[cfg(windows)]
    {
        command = Command::new("cmd");
        command.arg("/C").arg(editor);
    }
    command.arg(file);
    command
}

pub fn repo_init(context: &CommandContext, force: bool) -> Result<i32> {
    let repository = discover_repository(&context.cwd)?;
    let target = repository.top_level.join(".acre.json");
    if !force && target.exists() {
        return Err(AcreError::new(
            "ACRE_REPO_CONFIG_EXISTS",
            ".acre.json already exists.",
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "path": target })));
    }
    write_default_repo_config(&target)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "path": target}));
    } else {
        renderer.line(format!("<green>Created</green> <dim>{}</dim>", target.display()));
    }
    Ok(exit::SUCCESS)
}
