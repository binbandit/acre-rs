//! Renders an error for humans or as JSON, with a hint for the common cases.

use crate::cli::CommandContext;
use crate::error::AcreError;
use crate::ui::output::Renderer;

pub fn render_failure(context: &CommandContext, error: &AcreError) -> i32 {
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({
            "ok": false,
            "error": {
                "code": error.code,
                "message": error.message,
                "details": error.details,
            }
        }));
        return error.exit_code;
    }
    renderer.error(format!(
        "<bold><red>{}</red></bold>",
        renderer.value(&error.message)
    ));
    if let Some(suggestions) = error
        .details
        .get("suggestions")
        .and_then(serde_json::Value::as_array)
    {
        if !suggestions.is_empty() {
            renderer.error("");
            renderer.error("Closest matches:");
            for suggestion in suggestions {
                renderer.error(format!(
                    "  <blue>{}</blue>",
                    renderer.value(suggestion.as_str().unwrap_or_default())
                ));
            }
        }
    }
    if let Some(matches) = error.details.get("matches").and_then(serde_json::Value::as_array) {
        if !matches.is_empty() {
            renderer.error("");
            for candidate in matches {
                renderer.error(format!(
                    "  <blue>{}</blue>",
                    renderer.value(candidate.as_str().unwrap_or_default())
                ));
            }
        }
    }
    // The one place we suggest creating a branch: an unknown target is never created silently.
    if error.code == "ACRE_TARGET_NOT_FOUND" {
        let selector = error
            .details
            .get("selector")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("branch");
        renderer.error("");
        renderer.error("Start a new branch:");
        renderer.error(format!("  <blue>acre new {}</blue>", renderer.value(selector)));
    }
    if error.code == "ACRE_BRANCH_EXISTS" {
        let branch = error
            .details
            .get("branch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("branch");
        renderer.error("");
        renderer.error("Open it:");
        renderer.error(format!("  <blue>acre {}</blue>", renderer.value(branch)));
    }
    if context.global.verbose && !error.details.as_object().is_none_or(serde_json::Map::is_empty) {
        renderer.error("");
        renderer.error(format!(
            "<dim>{}</dim>",
            renderer.value(error.details.to_string())
        ));
    }
    error.exit_code
}
