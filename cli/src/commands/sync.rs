use std::io::Write;

use shaic_core::adapters;
use shaic_core::config::Config;
use shaic_core::materialize;
use shaic_core::model::{AgentId, Scope};

use crate::error::Result;

use super::{current_project_root, open_store};

#[allow(clippy::too_many_arguments)]
pub fn run(
    agents: Vec<AgentId>,
    global: bool,
    project: bool,
    all: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    let store = open_store()?;
    let mut config = Config::load()?;
    let project_root = current_project_root()?;

    let scopes: Vec<Scope> = if all || (!global && !project) {
        vec![Scope::Global, Scope::Project]
    } else {
        let mut s = Vec::new();
        if global {
            s.push(Scope::Global);
        }
        if project {
            s.push(Scope::Project);
        }
        s
    };

    if scopes.contains(&Scope::Project) {
        config.ensure_project_registered(&project_root)?;
    }

    let target_agents: Vec<AgentId> = if agents.is_empty() {
        config.enabled_agent_ids()
    } else {
        agents
    };

    // Pull every targeted agent's on-disk MCP servers back into the store
    // before planning or applying anything, in its own pass over every
    // agent — so a server added or edited directly in one agent's config
    // (bypassing `shaic mcp add`/`edit`) reaches every *other* agent in
    // this same `sync` run regardless of which one happens to sort first.
    // Interleaving this per-agent inside the main loop below would only
    // fan a change out to agents processed *after* the one that made it.
    // Skipped entirely on `--dry-run`: unlike everything else here, this
    // writes into the store immediately rather than returning a plan to
    // review first.
    if !dry_run {
        for &id in &target_agents {
            let agent = adapters::by_id(id);
            for &scope in &scopes {
                let report =
                    materialize::reconcile_mcp(agent.as_ref(), &store, scope, &project_root);
                print_reconcile_report(report, agent.display_name(), scope, "MCP");

                // Unlike MCP (independent of `supported_scopes()`, gated by
                // `mcp_target` internally), item kinds only ever exist in
                // scopes the agent actually supports — reconciling outside
                // that would ask e.g. Cursor's Project-only `root()` for
                // "Global" content and just re-discover the same Project
                // files under the wrong scope.
                if agent.supported_scopes().contains(&scope) {
                    for &kind in agent.supported_kinds() {
                        let report = materialize::reconcile_items(
                            agent.as_ref(),
                            &store,
                            kind,
                            scope,
                            &project_root,
                        );
                        print_reconcile_report(
                            report,
                            agent.display_name(),
                            scope,
                            &format!("{kind:?}"),
                        );
                    }
                }
            }
        }
    }

    let mut any_changes = false;
    for id in target_agents {
        let agent = adapters::by_id(id);
        for &scope in &scopes {
            if agent.supported_scopes().contains(&scope) {
                let plan =
                    materialize::plan_materialize(agent.as_ref(), &store, scope, &project_root)?;
                for note in &plan.skipped {
                    println!("[skip] {note}");
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
                            materialize::apply(agent.as_ref(), &plan, scope, &project_root)?;
                            println!("  applied.");
                        } else {
                            println!("  skipped (not confirmed)");
                        }
                    }
                }
            }

            // MCP scope support is independent of `supported_scopes()` — an
            // agent can have MCP servers in a different set of scopes than
            // it has skills/rules/commands in. A planning error here (e.g. a
            // hand-edited config with an unexpected shape at the managed key)
            // must not abort materialization for every other agent/scope in
            // this run.
            let mcp_plan = match materialize::plan_mcp(agent.as_ref(), &store, scope, &project_root)
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
            let mcp_changed: Vec<_> = mcp_plan.changed_writes().collect();
            if !mcp_changed.is_empty() || !mcp_plan.removals.is_empty() {
                any_changes = true;
                println!("== {} / {scope:?} (MCP) ==", agent.display_name());
                for w in &mcp_changed {
                    println!("  {:?} {}", w.action, w.name);
                }
                for name in &mcp_plan.removals {
                    println!("  Remove {name}");
                }
                if !dry_run {
                    if yes || confirm("Apply these MCP changes?")? {
                        materialize::apply_mcp(
                            agent.as_ref(),
                            &store,
                            &mcp_plan,
                            scope,
                            &project_root,
                        )?;
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

fn print_reconcile_report(
    result: shaic_core::error::Result<materialize::ReconcileReport>,
    agent_name: &str,
    scope: Scope,
    tag: &str,
) {
    match result {
        Ok(report) => {
            for name in &report.pulled {
                println!("[pulled] {name:?} from {agent_name} / {scope:?} ({tag})");
            }
            for (name, reason) in &report.rejected {
                println!(
                    "[skip] could not pull {name:?} from {agent_name} / {scope:?} ({tag}): {reason}"
                );
            }
        }
        Err(e) => println!("[skip] reconcile {agent_name} / {scope:?} ({tag}): {e}"),
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
