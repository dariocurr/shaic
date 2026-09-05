use shaic_core::operations;

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
    force: bool,
) -> Result<()> {
    let store = open_store()?;
    let targets = resolve_targets(agents, global, project, all, false)?;

    if !yes && !confirm("Pull agent on-disk files into the store? Store will be written.")? {
        println!("aborted — store untouched");
        return Ok(());
    }

    let mut any = false;
    for &id in &targets.agents {
        let agent = shaic_core::adapters::by_id(id);
        for &scope in &targets.scopes {
            let summary =
                operations::import_scope(agent, &store, scope, &targets.project_root, force);
            // Same summary shape the TUI uses: pulled/rejected first, then
            // collected per-kind errors (previously swallowed in TUI with
            // `if let Ok`, printed as `[skip]` here).
            for name in &summary.pulled {
                println!(
                    "[pulled] {name:?} from {} / {scope:?}",
                    agent.display_name()
                );
                any = true;
            }
            for (name, reason) in &summary.rejected {
                println!(
                    "[skip] could not pull {name:?} from {} / {scope:?}: {reason}",
                    agent.display_name()
                );
                any = true;
            }
            for note in &summary.warnings {
                println!("[warn] {} / {scope:?}: {note}", agent.display_name());
            }
            for err in &summary.errors {
                println!("[skip] {err}");
                any = true;
            }
        }
    }

    if !any {
        println!("nothing new to import.");
    }
    Ok(())
}
