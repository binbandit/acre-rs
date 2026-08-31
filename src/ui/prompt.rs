use std::io::{self, Write};

use crate::error::{AcreError, Result, exit};
use crate::ui::output::Renderer;

pub fn confirm(renderer: &Renderer<'_>, question: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{} ", renderer.format(&format!("{question} {suffix}"), true));
    io::stdout().flush().map_err(|error| AcreError::io("could not write prompt", error))?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).map_err(|error| AcreError::io("could not read prompt", error))?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer.is_empty() {
        return Ok(default_yes);
    }
    match answer.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Err(AcreError::new("ACRE_PROMPT_INVALID", "Enter yes or no.", exit::USAGE)),
    }
}
