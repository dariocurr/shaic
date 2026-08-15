use std::io::IsTerminal;

use serde::Serialize;

use crate::error::Result;
use shaic_core::adapters;
use shaic_core::materialize;
use shaic_core::model::{AgentId, Scope};

use super::{current_project_root, open_store};

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

#[derive(Serialize)]
struct StatusReport {
    store: String,
    uncommitted_changes: usize,
    project_root: String,
    agents: Vec<AgentEntry>,
}

#[derive(Serialize)]
struct AgentEntry {
    agent: AgentId,
    scope: Scope,
    kind: &'static str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn colorize(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn sync_label(in_sync: bool, color: bool) -> String {
    if in_sync {
        colorize("in-sync", GREEN, color)
    } else {
        colorize("drift", YELLOW, color)
    }
}

pub fn run(json: bool) -> Result<()> {
    if json {
        return run_json();
    }

    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();

    let store = open_store()?;
    let status = shaic_core::store::git::status_porcelain(store.root())?;
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    let dirty_label = colorize(
        &format!("{dirty} uncommitted change(s)"),
        if dirty == 0 { GREEN } else { YELLOW },
        color,
    );
    println!("store: {} ({dirty_label})", store.root().display());

    let project_root = current_project_root()?;
    println!("\nagents (project: {}):", project_root.display());
    for &agent in adapters::registry() {
        for &scope in &[Scope::Global, Scope::Project] {
            if scope == Scope::Project && !project_root.exists() {
                continue;
            }

            if agent.supported_scopes().contains(&scope) {
                if agent.experimental_read_only() {
                    println!(
                        "  {:<20} {:?}  [{}]",
                        agent.display_name(),
                        scope,
                        colorize("experimental, read-only", MAGENTA, color)
                    );
                } else {
                    match materialize::plan_materialize(agent, &store, scope, &project_root) {
                        Ok(plan) => {
                            let glyph = sync_label(plan.is_empty(), color);
                            println!("  {:<20} {:?}  [{glyph}]", agent.display_name(), scope);
                        }
                        Err(e) => {
                            println!(
                                "  {:<20} {:?}  [{}]",
                                agent.display_name(),
                                scope,
                                colorize(&format!("error: {e}"), RED, color)
                            );
                        }
                    }
                }
            }

            if agent.mcp_target(scope, &project_root).is_none() {
                continue;
            }
            match materialize::plan_mcp(agent, &store, scope, &project_root) {
                Ok(mcp_plan) => {
                    let glyph = sync_label(mcp_plan.is_empty(), color);
                    println!(
                        "  {:<20} {:?}  [{glyph}] (mcp)",
                        agent.display_name(),
                        scope
                    );
                }
                Err(e) => {
                    println!(
                        "  {:<20} {:?}  [{}] (mcp)",
                        agent.display_name(),
                        scope,
                        colorize(&format!("error: {e}"), RED, color)
                    );
                }
            }
        }
    }
    Ok(())
}

fn run_json() -> Result<()> {
    let store = open_store()?;
    let status = shaic_core::store::git::status_porcelain(store.root())?;
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count();
    let project_root = current_project_root()?;
    let mut agents = Vec::new();

    for &agent in adapters::registry() {
        for &scope in &[Scope::Global, Scope::Project] {
            if scope == Scope::Project && !project_root.exists() {
                continue;
            }

            if agent.supported_scopes().contains(&scope) {
                if agent.experimental_read_only() {
                    agents.push(AgentEntry {
                        agent: agent.id(),
                        scope,
                        kind: "items",
                        state: "experimental",
                        error: None,
                    });
                } else {
                    match materialize::plan_materialize(agent, &store, scope, &project_root) {
                        Ok(plan) => agents.push(AgentEntry {
                            agent: agent.id(),
                            scope,
                            kind: "items",
                            state: if plan.is_empty() { "in-sync" } else { "drift" },
                            error: None,
                        }),
                        Err(e) => agents.push(AgentEntry {
                            agent: agent.id(),
                            scope,
                            kind: "items",
                            state: "error",
                            error: Some(e.to_string()),
                        }),
                    }
                }
            }

            if agent.mcp_target(scope, &project_root).is_none() {
                continue;
            }
            match materialize::plan_mcp(agent, &store, scope, &project_root) {
                Ok(mcp_plan) => agents.push(AgentEntry {
                    agent: agent.id(),
                    scope,
                    kind: "mcp",
                    state: if mcp_plan.is_empty() {
                        "in-sync"
                    } else {
                        "drift"
                    },
                    error: None,
                }),
                Err(e) => agents.push(AgentEntry {
                    agent: agent.id(),
                    scope,
                    kind: "mcp",
                    state: "error",
                    error: Some(e.to_string()),
                }),
            }
        }
    }

    let report = StatusReport {
        store: store.root().display().to_string(),
        uncommitted_changes: dirty,
        project_root: project_root.display().to_string(),
        agents,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
