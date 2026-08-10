use std::io::IsTerminal;

use crate::error::Result;
use shaic_core::adapters;
use shaic_core::materialize;
use shaic_core::model::Scope;

use super::{current_project_root, open_store};

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

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

pub fn run() -> Result<()> {
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
    for agent in adapters::registry() {
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
                    // Same reasoning as the MCP planning error below: one
                    // unreadable store item must not abort status reporting
                    // for every other agent/scope.
                    match materialize::plan_materialize(
                        agent.as_ref(),
                        &store,
                        scope,
                        &project_root,
                    ) {
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
            // A planning error for one agent/scope (e.g. a hand-edited config
            // with an unexpected shape at the managed key) must not abort
            // status reporting for every other agent/scope.
            match materialize::plan_mcp(agent.as_ref(), &store, scope, &project_root) {
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
