use shaic_core::adapters;
use shaic_core::materialize;

use crate::error::Result;

use super::{confirm, open_store, resolve_targets};

/// Materialize the canonical store out to agent config files.
/// Does not pull agent files into the store — that is `shaic import`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    agents: Vec<shaic_core::model::AgentId>,
    global: bool,
    project: bool,
    all: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let store = open_store()?;
    let targets = resolve_targets(agents, global, project, all, !dry_run)?;

    let mut any_changes = false;
    for id in targets.agents {
        let agent = adapters::by_id(id);
        for &scope in &targets.scopes {
            if agent.supported_scopes().contains(&scope) {
                let plan =
                    materialize::plan_materialize(agent, &store, scope, &targets.project_root)?;
                for note in &plan.skipped {
                    println!("[skip] {note}");
                }
                for note in &plan.warnings {
                    println!("[warn] {note}");
                }
                let changed: Vec<_> = plan.changed_writes().collect();
                if !changed.is_empty() || !plan.deletes.is_empty() {
                    any_changes = true;
                    println!("== {} / {scope:?} ==", agent.display_name());
                    for w in &changed {
                        println!("  {:?} {}", w.action, w.relative_path.display());
                    }
                    for d in &plan.deletes {
                        println!("  Delete {}", d.relative_path.display());
                    }
                    if !dry_run {
                        if yes || confirm("Apply these changes?")? {
                            let report =
                                materialize::apply(agent, &plan, scope, &targets.project_root)?;
                            for note in &report.warnings {
                                println!("[warn] {note}");
                            }
                            println!("  applied.");
                        } else {
                            println!("  skipped (not confirmed)");
                        }
                    }
                }
            }

            let mcp_plan = match materialize::plan_mcp(agent, &store, scope, &targets.project_root)
            {
                Ok(plan) => plan,
                Err(e) => {
                    println!("[skip] {} / {scope:?} (MCP): {e}", agent.display_name());
                    continue;
                }
            };
            for note in &mcp_plan.skipped {
                println!("[skip] {note}");
            }
            for note in &mcp_plan.warnings {
                println!("[warn] {note}");
            }
            let mcp_changed: Vec<_> = mcp_plan.changed_writes().collect();
            if !mcp_changed.is_empty() || !mcp_plan.removals.is_empty() {
                any_changes = true;
                println!("== {} / {scope:?} (MCP) ==", agent.display_name());
                for w in &mcp_changed {
                    if w.summary.is_empty() {
                        println!("  {:?} {}", w.action, w.name);
                    } else {
                        println!("  {:?} {} ({})", w.action, w.name, w.summary);
                    }
                }
                for name in &mcp_plan.removals {
                    println!("  Remove {name}");
                }
                if !dry_run {
                    if yes || confirm("Apply these MCP changes?")? {
                        let report = materialize::apply_mcp(
                            agent,
                            &store,
                            &mcp_plan,
                            scope,
                            &targets.project_root,
                        )?;
                        for note in &report.warnings {
                            println!("[warn] {note}");
                        }
                        println!("  applied.");
                    } else {
                        println!("  skipped (not confirmed)");
                    }
                }
            }
        }
    }

    if !any_changes {
        println!("everything is already in sync.");
    }
    Ok(())
}
