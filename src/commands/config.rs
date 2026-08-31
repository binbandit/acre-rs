use std::fs;
use std::process::{Command, Stdio};

use crate::error::{AcreError, Result, exit};
use crate::git::repository::discover_repository;
use crate::model::CommandContext;
use crate::state::config::{
    default_config, load_config, save_config, set_config_value, validate_acre_config,
    write_default_repo_config,
};
use crate::state::paths::config_path;
use crate::ui::output::Renderer;

pub fn command_config_show(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&config);
    } else {
        renderer.raw(format!("{}\n", serde_json::to_string_pretty(&config)?));
    }
    Ok(exit::SUCCESS)
}

pub fn command_config_path(context: &CommandContext) -> Result<i32> {
    Renderer::new(context).raw(format!("{}\n", config_path().display()));
    Ok(exit::SUCCESS)
}

pub fn command_config_init(context: &CommandContext, force: bool) -> Result<i32> {
    let target = config_path();
    if !force && target.exists() {
        return Err(AcreError::new(
            "ACRE_CONFIG_EXISTS",
            "Acre configuration already exists.",
            exit::CONFLICT,
        )
        .with_details(serde_json::json!({ "path": target })));
    }
    save_config(&default_config())?;
    Renderer::new(context).line(format!("<green>Created</green> <dim>{}</dim>", target.display()));
    Ok(exit::SUCCESS)
}

pub fn command_config_set(context: &CommandContext, key: &str, value: &str) -> Result<i32> {
    let mut config = load_config()?;
    let parsed = set_config_value(&mut config, key, value)?;
    validate_acre_config(&config)?;
    save_config(&config)?;
    let renderer = Renderer::new(context);
    renderer.line(format!("<green>Set</green> <blue>{}</blue> = {}", renderer.value(key), renderer.value(parsed.to_string())));
    Ok(exit::SUCCESS)
}

pub fn command_config_edit(_context: &CommandContext) -> Result<i32> {
    let target = config_path();
    if !target.exists() {
        save_config(&default_config())?;
    }
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).map_err(|_| {
        AcreError::new(
            "ACRE_EDITOR_NOT_CONFIGURED",
            "Set $VISUAL or $EDITOR before using acre config edit.",
            exit::ENVIRONMENT,
        )
    })?;
    let mut pieces = split_command(&editor);
    let program = pieces.first().cloned().unwrap_or(editor);
    if !pieces.is_empty() { pieces.remove(0); }
    let status = Command::new(program)
        .args(pieces)
        .arg(target)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| AcreError::io("could not start editor", error))?;
    Ok(status.code().unwrap_or(exit::INTERNAL))
}

pub fn command_repo_config_init(context: &CommandContext, force: bool) -> Result<i32> {
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
    Renderer::new(context).line(format!("<green>Created</green> <dim>{}</dim>", target.display()));
    Ok(exit::SUCCESS)
}

fn split_command(value: &str) -> Vec<String> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in value.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    output.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() { output.push(current); }
    output
}
