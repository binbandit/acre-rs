use crate::error::{Result, exit};
use crate::model::{CommandContext, SupportedShell};
use crate::shell::generator::{generate_completion, generate_shell_integration};
use crate::ui::output::Renderer;

pub fn command_shell_init(context: &CommandContext, shell: SupportedShell) -> Result<i32> {
    Renderer::new(context).raw(generate_shell_integration(shell));
    Ok(exit::SUCCESS)
}

pub fn command_completion(context: &CommandContext, shell: SupportedShell) -> Result<i32> {
    Renderer::new(context).raw(format!("{}\n", generate_completion(shell)));
    Ok(exit::SUCCESS)
}
