use crate::error::Result;
use shaic_core::adapters;

use crate::AgentsAction;

use super::current_project_root;

pub fn run(action: AgentsAction) -> Result<()> {
    match action {
        AgentsAction::List => {
            for &agent in adapters::registry() {
                let scopes = agent
                    .supported_scopes()
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join("+");
                let kinds = agent
                    .supported_kinds()
                    .iter()
                    .map(|k| format!("{k:?}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let note = if agent.experimental_read_only() {
                    "  [experimental, read-only]"
                } else {
                    ""
                };
                println!(
                    "{:<12} {:<24} scopes={scopes:<15} kinds={kinds}{note}",
                    agent.id().as_str(),
                    agent.display_name()
                );
            }
        }
        AgentsAction::Discover { agent } => {
            let project_root = current_project_root()?;
            for summary in shaic_core::discovery::discover_all(&project_root) {
                if let Some(only) = agent
                    && summary.agent != only
                {
                    continue;
                }
                for found in summary.found {
                    println!(
                        "{}\t{:?}\t{:?}\t{}",
                        summary.agent.as_str(),
                        summary.kind,
                        summary.scope,
                        found.source_path.display()
                    );
                }
            }
        }
    }
    Ok(())
}
