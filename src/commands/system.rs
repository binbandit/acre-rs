use crate::error::{Result, exit};
use crate::git::repository::discover_repository;
use crate::git::runner::{RunOptions, decode_stdout, run_process};
use crate::model::{CommandContext, WorkspaceStatus};
use crate::pool::broker::warm_repository;
use crate::pool::maintenance::{gc_repository, repair_repository_state};
use crate::state::config::load_config;
use crate::state::repository::load_repository_state;
use crate::ui::output::Renderer;

pub fn command_system_warm(context: &CommandContext, slots: Option<usize>) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let created = warm_repository(&config, &repository, slots)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "created": created.len(), "slots": created }));
    } else if created.is_empty() {
        renderer.line("<green>The warm workspace pool is ready.</green>");
    } else {
        renderer.line(format!("<bold><green>Prepared {}</green></bold> warm workspace{}", created.len(), if created.len() == 1 { "" } else { "s" }));
        for slot in &created {
            renderer.line(format!("  <dim>{}</dim> · {:?}", renderer.value(slot.path.display().to_string()), slot.environment.as_ref().map(|environment| environment.state).unwrap_or(crate::model::EnvironmentState::Cold)));
        }
    }
    Ok(exit::SUCCESS)
}

pub fn command_system_inspect(context: &CommandContext) -> Result<i32> {
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
    if state.workspaces.is_empty() { renderer.line("  <dim>none</dim>"); }
    for workspace in &state.workspaces {
        renderer.line(format!("  <blue>{}</blue>  {:?} · {:?}", renderer.value(&workspace.target.display_name), workspace.ownership, workspace.environment.as_ref().map(|environment| environment.state).unwrap_or(crate::model::EnvironmentState::Unknown)));
        renderer.line(format!("    <dim>{}</dim>", renderer.value(workspace.path.display().to_string())));
    }
    renderer.line("");
    renderer.line("Warm slots");
    let idle = state.slots.iter().filter(|slot| slot.status == WorkspaceStatus::Idle).collect::<Vec<_>>();
    if idle.is_empty() { renderer.line("  <dim>none</dim>"); }
    for slot in idle {
        renderer.line(format!("  {}  {:?} · {}", slot.id, slot.environment.as_ref().map(|environment| environment.state).unwrap_or(crate::model::EnvironmentState::Cold), slot.environment.as_ref().map(|environment| &environment.fingerprint[..environment.fingerprint.len().min(8)]).unwrap_or("no fingerprint")));
    }
    renderer.line("");
    renderer.line(format!("Leases  {}", state.leases.len()));
    for lease in &state.leases {
        renderer.line(format!("  {}  {}", &lease.id[..lease.id.len().min(8)], renderer.value(&lease.holder)));
    }
    Ok(exit::SUCCESS)
}

#[derive(serde::Serialize)]
struct DoctorCheck {
    name: String,
    ok: bool,
    detail: String,
}

pub fn command_system_doctor(context: &CommandContext) -> Result<i32> {
    let renderer = Renderer::new(context);
    let mut checks = Vec::new();
    match run_process("git", &["--version".into()], RunOptions::default()) {
        Ok(result) => checks.push(DoctorCheck { name: "Git".into(), ok: true, detail: decode_stdout(&result) }),
        Err(error) => checks.push(DoctorCheck { name: "Git".into(), ok: false, detail: error.to_string() }),
    }
    let config = load_config()?;
    checks.push(DoctorCheck { name: "Configuration".into(), ok: true, detail: config.root.display().to_string() });
    match discover_repository(&context.cwd) {
        Ok(repository) => match load_repository_state(&config, &repository) {
            Ok(state) => {
                let broken = state.workspaces.iter().filter(|workspace| workspace.status == WorkspaceStatus::Broken).count()
                    + state.slots.iter().filter(|slot| slot.status == WorkspaceStatus::Broken).count();
                let idle = state.slots.iter().filter(|slot| slot.status == WorkspaceStatus::Idle).count();
                checks.push(DoctorCheck { name: "Repository".into(), ok: true, detail: repository.top_level.display().to_string() });
                checks.push(DoctorCheck { name: "Acre state".into(), ok: broken == 0, detail: if broken == 0 { "consistent".into() } else { format!("{broken} broken record(s)") } });
                checks.push(DoctorCheck { name: "Warm pool".into(), ok: idle > 0, detail: format!("{idle} idle slot(s)") });
            }
            Err(error) => checks.push(DoctorCheck { name: "Acre state".into(), ok: false, detail: error.to_string() }),
        },
        Err(error) => checks.push(DoctorCheck { name: "Repository".into(), ok: false, detail: error.to_string() }),
    }
    let ok = checks.iter().all(|check| check.ok);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": ok, "checks": checks }));
    } else {
        renderer.line("<bold>Acre doctor</bold>");
        renderer.line("");
        for check in &checks {
            renderer.line(format!("{} {}  <dim>{}</dim>", if check.ok { "<green>✓</green>" } else { "<red>✗</red>" }, renderer.value(&check.name), renderer.value(&check.detail)));
        }
        if !ok {
            renderer.line("");
            renderer.line("Run <blue>acre system repair</blue> inside the affected repository.");
        }
    }
    Ok(if ok { exit::SUCCESS } else { exit::ENVIRONMENT })
}

pub fn command_system_repair(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let report = repair_repository_state(&config, &repository)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "report": report }));
    } else {
        renderer.line("<bold><green>Reconciled Acre state</green></bold>");
        renderer.line(format!("  recovered slots       {}", report.added_slots));
        renderer.line(format!("  recovered workspaces  {}", report.added_workspaces));
        renderer.line(format!("  removed stale records {}", report.removed_broken_records));
    }
    Ok(exit::SUCCESS)
}

pub fn command_system_gc(context: &CommandContext) -> Result<i32> {
    let config = load_config()?;
    let repository = discover_repository(&context.cwd)?;
    let report = gc_repository(&config, &repository)?;
    let renderer = Renderer::new(context);
    if context.global.json {
        renderer.json(&serde_json::json!({ "ok": true, "removed": report.removed, "skipped": report.skipped }));
    } else {
        renderer.line(format!("<bold><green>Removed {}</green></bold> idle workspace{}", report.removed.len(), if report.removed.len() == 1 { "" } else { "s" }));
        for skipped in &report.skipped {
            renderer.line(format!("  <yellow>kept {}</yellow> · {}", skipped.slot.id, renderer.value(&skipped.reason)));
        }
    }
    Ok(exit::SUCCESS)
}
