pub mod mcp;
pub mod plan;
pub mod writer;

pub use mcp::{
    McpApplyReport, McpPlan, PlannedMcpWrite, ReconcileReport, apply_mcp, plan_mcp, reconcile_mcp,
};
pub use plan::{
    ApplyReport, MaterializePlan, PlannedDelete, PlannedWrite, apply, plan_materialize,
    reconcile_items,
};
pub use writer::WriteAction;

use std::path::Path;

use crate::adapters;
use crate::model::Scope;
use crate::store::Store;

/// Materializes and applies every agent's pending plan immediately, with no
/// confirmation and — unlike `sync` — no prior reconcile pass: the caller
/// just changed the store on purpose (e.g. deleted an item) and reconciling
/// first would just re-discover the stale on-disk copy and undo that change.
/// Tolerant of per-agent/scope errors: an unrelated agent's broken config
/// shouldn't block the push everywhere else, so each failure becomes a note
/// in the returned list rather than aborting the whole pass. Returns the
/// count of agent/scope plans actually applied, plus any notes.
pub fn push_all_now(store: &Store, project_root: &Path) -> (usize, Vec<String>) {
    let mut applied = 0;
    let mut notes = Vec::new();
    for &agent in adapters::registry() {
        for &scope in &[Scope::Global, Scope::Project] {
            if agent.supported_scopes().contains(&scope) && !agent.experimental_read_only() {
                match plan_materialize(agent, store, scope, project_root) {
                    Ok(plan) if !plan.is_empty() => {
                        match apply(agent, &plan, scope, project_root) {
                            Ok(_) => applied += 1,
                            Err(e) => notes.push(format!(
                                "{} / {scope:?}: apply failed: {e}",
                                agent.display_name()
                            )),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => notes.push(format!(
                        "{} / {scope:?}: plan failed: {e}",
                        agent.display_name()
                    )),
                }
            }

            match plan_mcp(agent, store, scope, project_root) {
                Ok(plan) if !plan.is_empty() => {
                    match apply_mcp(agent, store, &plan, scope, project_root) {
                        Ok(_) => applied += 1,
                        Err(e) => notes.push(format!(
                            "{} / {scope:?} (mcp): apply failed: {e}",
                            agent.display_name()
                        )),
                    }
                }
                Ok(_) => {}
                Err(e) => notes.push(format!(
                    "{} / {scope:?} (mcp): plan failed: {e}",
                    agent.display_name()
                )),
            }
        }
    }
    (applied, notes)
}
