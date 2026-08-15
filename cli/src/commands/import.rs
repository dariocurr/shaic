use shaic_core::adapters;
use shaic_core::materialize;
use shaic_core::model::Scope;

use crate::error::Result;

use super::{confirm, open_store, resolve_targets};

/// Pull agent on-disk items and MCP servers into the canonical store.
/// Does not materialize anything back out — that is `shaic sync`.
pub fn run(
    agents: Vec<shaic_core::model::AgentId>,
    global: bool,
    project: bool,
    all: bool,
    yes: bool,
) -> Result<()> {
    let store = open_store()?;
    let targets = resolve_targets(agents, global, project, all, false)?;

    if !yes && !confirm("Pull agent on-disk files into the store? Store will be written.")? {
        println!("aborted — store untouched");
        return Ok(());
    }

    let mut any = false;
    for &id in &targets.agents {
        let agent = adapters::by_id(id);
        for &scope in &targets.scopes {
            let report = materialize::reconcile_mcp(agent, &store, scope, &targets.project_root);
            any |= print_reconcile_report(report, agent.display_name(), scope, "MCP");

            if agent.supported_scopes().contains(&scope) {
                for &kind in agent.supported_kinds() {
                    let report = materialize::reconcile_items(
                        agent,
                        &store,
                        kind,
                        scope,
                        &targets.project_root,
                    );
                    any |= print_reconcile_report(
                        report,
                        agent.display_name(),
                        scope,
                        &format!("{kind:?}"),
                    );
                }
            }
        }
    }

    if !any {
        println!("nothing new to import.");
    }
    Ok(())
}

fn print_reconcile_report(
    result: shaic_core::error::Result<materialize::ReconcileReport>,
    agent_name: &str,
    scope: Scope,
    tag: &str,
) -> bool {
    match result {
        Ok(report) => {
            let mut any = !report.pulled.is_empty();
            for name in &report.pulled {
                println!("[pulled] {name:?} from {agent_name} / {scope:?} ({tag})");
            }
            for (name, reason) in &report.rejected {
                any = true;
                println!(
                    "[skip] could not pull {name:?} from {agent_name} / {scope:?} ({tag}): {reason}"
                );
            }
            for note in &report.warnings {
                println!("[warn] {agent_name} / {scope:?} ({tag}): {note}");
            }
            any
        }
        Err(e) => {
            println!("[skip] import {agent_name} / {scope:?} ({tag}): {e}");
            true
        }
    }
}
