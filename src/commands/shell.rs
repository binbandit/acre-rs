//! `acre shell init` and `acre completion`.

use crate::cli::CommandContext;
use crate::error::{Result, exit};
use crate::shell::SupportedShell;
use crate::shell::generator::{generate_completion, generate_shell_integration};
use crate::ui::output::Renderer;

pub fn init(context: &CommandContext, shell: SupportedShell) -> Result<i32> {
    let script = generate_shell_integration(shell);
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "shell": shell, "script": script}));
    } else {
        renderer.raw(script);
    }
    Ok(exit::SUCCESS)
}

pub fn completion(context: &CommandContext, shell: SupportedShell) -> Result<i32> {
    let script = generate_completion(shell);
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({"ok": true, "shell": shell, "script": script}));
    } else {
        renderer.raw(format!("{script}\n"));
    }
    Ok(exit::SUCCESS)
}
