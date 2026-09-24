//! `acre system`: warm, inspect, doctor, repair, gc.

use crate::cli::CommandContext;
use crate::error::{Result, exit};
use crate::git::repository::discover_repository;
use crate::git::runner::git_version;
use crate::model::EnvironmentState;
use crate::model::WorkspaceStatus;
use crate::state::config::load_config;
use crate::state::repository::load_repository_state;
use crate::ui::output::Renderer;
use crate::util::display_path;
use crate::workspace::maintain::{gc_repository, repair_repository_state};
use crate::workspace::pool::warm_repository;

pub fn warm(context: &CommandContext, slots: Option<usize>) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let created = warm_repository(&config, &repository, slots)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "created": created.len(), "slots": created }));
    } else if created.is_empty() {
        renderer.line("<green>The warm workspace pool is ready.</green>");
    } else {
        renderer.line(format!(
            "<bold><green>Prepared {}</green></bold> warm workspace{}",
            created.len(),
            if created.len() == 1 { "" } else { "s" }
        ));
        for slot in &created {
            renderer.line(format!(
                "  <dim>{}</dim> · {}",
                renderer.value(display_path(&slot.path)),
                slot.environment
                    .as_ref()
                    .map(|environment| environment.state)
                    .unwrap_or(EnvironmentState::Cold)
            ));
        }
    }
    Ok(exit::SUCCESS)
}

pub fn inspect(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let state = load_repository_state(&config, &repository)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "repository": repository, "state": state }));
        return Ok(exit::SUCCESS);
    }
    renderer.line(format!("<bold>{}</bold>", renderer.value(&repository.name)));
    renderer.line("");
    renderer.line("Active workspaces");
    if state.workspaces.is_empty() {
        renderer.line("  <dim>none</dim>");
    }
    for workspace in &state.workspaces {
        renderer.line(format!(
            "  <blue>{}</blue>  {} · {}",
            renderer.value(&workspace.target.display_name),
            workspace.ownership,
            workspace
                .environment
                .as_ref()
                .map(|environment| environment.state)
                .unwrap_or(EnvironmentState::Unknown)
        ));
        renderer.line(format!(
            "    <dim>{}</dim>",
            renderer.value(display_path(&workspace.path))
        ));
    }
    renderer.line("");
    renderer.line("Warm slots");
    let idle = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .collect::<Vec<_>>();
    if idle.is_empty() {
        renderer.line("  <dim>none</dim>");
    }
    for slot in idle {
        renderer.line(format!(
            "  {}  {} · {}",
            slot.id,
            slot.environment
                .as_ref()
                .map(|environment| environment.state)
                .unwrap_or(EnvironmentState::Cold),
            slot.environment
                .as_ref()
                .map(|environment| &environment.fingerprint[..environment.fingerprint.len().min(8)])
                .unwrap_or("no fingerprint")
        ));
    }
    renderer.line("");
    renderer.line(format!("Leases  {}", state.leases.len()));
    for lease in &state.leases {
        renderer.line(format!(
            "  {}  {}",
            &lease.id[..lease.id.len().min(8)],
            renderer.value(&lease.holder)
        ));
    }
    Ok(exit::SUCCESS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum CheckLevel {
    Pass,
    // Worth knowing, but nothing is broken.
    Warn,
    Fail,
}

#[derive(serde::Serialize)]
struct DoctorCheck {
    name: &'static str,
    ok: bool,
    level: CheckLevel,
    detail: String,
    /// What to run when this check is not a pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'static str>,
}

impl DoctorCheck {
    fn new(name: &'static str, level: CheckLevel, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: level != CheckLevel::Fail,
            level,
            detail: detail.into(),
            hint: None,
        }
    }

    fn hint(self, hint: &'static str) -> Self {
        Self {
            hint: Some(hint),
            ..self
        }
    }
}

pub fn doctor(context: &CommandContext) -> Result<i32> {
    let renderer = Renderer::new(context);
    let checks = doctor_checks(context);
    let ok = checks.iter().all(|check| check.ok);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": ok, "checks": checks }));
    } else {
        renderer.line("<bold>Acre doctor</bold>");
        renderer.line("");
        let width = checks.iter().map(|check| check.name.len()).max().unwrap_or(0);
        for check in &checks {
            let mark = match check.level {
                CheckLevel::Pass => "<green>✓</green>",
                CheckLevel::Warn => "<yellow>!</yellow>",
                CheckLevel::Fail => "<red>✗</red>",
            };
            renderer.line(format!(
                "{mark} {:width$}  <dim>{}</dim>",
                check.name,
                renderer.value(&check.detail)
            ));
        }
        let hints = checks.iter().filter_map(|check| check.hint).collect::<Vec<_>>();
        if !hints.is_empty() {
            renderer.line("");
            for hint in hints {
                renderer.line(hint);
            }
        }
    }
    Ok(if ok { exit::SUCCESS } else { exit::ENVIRONMENT })
}

fn doctor_checks(context: &CommandContext) -> Vec<DoctorCheck> {
    // Git first: without it every other check is moot.
    let mut checks = vec![match git_version() {
        Ok(version) => DoctorCheck::new("Git", CheckLevel::Pass, version),
        Err(error) => {
            DoctorCheck::new("Git", CheckLevel::Fail, error.to_string()).hint("Install Git 2.36 or newer.")
        }
    }];
    let config = match load_config() {
        Ok(config) => {
            checks.push(DoctorCheck::new(
                "Configuration",
                CheckLevel::Pass,
                display_path(&config.root),
            ));
            config
        }
        // Everything below reads state through the configuration, so stop here.
        Err(error) => {
            checks.push(
                DoctorCheck::new("Configuration", CheckLevel::Fail, error.to_string())
                    .hint("Fix the file shown by <blue>acre config path</blue>, or reset it with <blue>acre config init --force</blue>."),
            );
            return checks;
        }
    };
    let repository = match discover_repository(&context.cwd) {
        Ok(repository) => repository,
        // Outside a repository there is simply nothing repository-specific to check.
        Err(_) => {
            checks.push(DoctorCheck::new(
                "Repository",
                CheckLevel::Warn,
                "not inside a Git worktree; repository checks skipped",
            ));
            return checks;
        }
    };
    checks.push(DoctorCheck::new(
        "Repository",
        CheckLevel::Pass,
        display_path(&repository.top_level),
    ));
    let state = match load_repository_state(&config, &repository) {
        Ok(state) => state,
        Err(error) => {
            checks.push(
                DoctorCheck::new("Acre state", CheckLevel::Fail, error.to_string())
                    .hint("Run <blue>acre system repair</blue> inside this repository."),
            );
            return checks;
        }
    };
    let broken = state
        .workspaces
        .iter()
        .map(|workspace| workspace.status)
        .chain(state.slots.iter().map(|slot| slot.status))
        .filter(|status| *status == WorkspaceStatus::Broken)
        .count();
    checks.push(if broken == 0 {
        DoctorCheck::new("Acre state", CheckLevel::Pass, "consistent")
    } else {
        DoctorCheck::new(
            "Acre state",
            CheckLevel::Fail,
            format!("{broken} broken record(s)"),
        )
        .hint("Run <blue>acre system repair</blue> inside this repository.")
    });
    let idle = state
        .slots
        .iter()
        .filter(|slot| slot.status == WorkspaceStatus::Idle)
        .count();
    // An empty pool only means the next `new` pays for a fresh checkout.
    checks.push(if idle == 0 {
        DoctorCheck::new("Warm pool", CheckLevel::Warn, "no idle slots")
            .hint("Prepare warm workspaces with <blue>acre system warm</blue>.")
    } else {
        DoctorCheck::new("Warm pool", CheckLevel::Pass, format!("{idle} idle slot(s)"))
    });
    checks
}

pub fn repair(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let report = repair_repository_state(&config, &repository)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "report": report }));
    } else {
        renderer.line("<bold><green>Reconciled Acre state</green></bold>");
        if let Some(path) = &report.quarantined_state {
            renderer.line(format!(
                "  unreadable state moved to <dim>{}</dim>",
                renderer.value(path.display().to_string())
            ));
        }
        renderer.line(format!("  recovered workspaces  {}", report.added_workspaces));
        renderer.line(format!(
            "  removed stale records {}",
            report.removed_broken_records
        ));
    }
    Ok(exit::SUCCESS)
}

pub fn gc(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let report = gc_repository(&config, &repository)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer
            .json(&serde_json::json!({ "ok": true, "removed": report.removed, "skipped": report.skipped }));
    } else {
        renderer.line(format!(
            "<bold><green>Removed {}</green></bold> idle workspace{}",
            report.removed.len(),
            if report.removed.len() == 1 { "" } else { "s" }
        ));
        for skipped in &report.skipped {
            renderer.line(format!(
                "  <yellow>kept {}</yellow> · {}",
                skipped.slot.id,
                renderer.value(&skipped.reason)
            ));
        }
    }
    Ok(exit::SUCCESS)
}
