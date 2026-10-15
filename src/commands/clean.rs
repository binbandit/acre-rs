//! `acre clean`: preview by default, apply only with --yes.

use crate::cli::CommandContext;
use crate::error::{Result, exit};
use crate::git::repository::discover_repository;
use crate::state::config::load_config;
use crate::ui::output::Renderer;
use crate::workspace::clean::clean_repository;

pub fn run(context: &CommandContext, base: Option<&str>, apply: bool) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let report = clean_repository(&config, &repository, base, apply, &std::env::current_dir()?)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "report": report }));
    } else {
        renderer.line(format!(
            "<bold>Cleanup against {}</bold>",
            renderer.value(&report.base)
        ));
        for item in &report.candidates {
            renderer.line(format!(
                "  Would {} {}",
                item.action.verb(false),
                renderer.value(&item.description)
            ));
        }
        for item in &report.cleaned {
            renderer.line(format!(
                "  <green>{}</green> {}",
                item.action.verb(true),
                renderer.value(&item.description)
            ));
        }
        for item in &report.skipped {
            renderer.line(format!(
                "  <dim>Kept</dim> {}: {}",
                renderer.value(&item.description),
                renderer.value(item.reason.as_deref().unwrap_or("unverified"))
            ));
        }
        if report.candidates.is_empty() && report.cleaned.is_empty() {
            renderer.line("Nothing to clean.");
        } else if report.dry_run {
            renderer.line("");
            renderer.line("Preview only. Run again with <blue>--yes</blue> to apply cleanup.");
        }
    }
    Ok(exit::SUCCESS)
}
