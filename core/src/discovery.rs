use std::path::Path;

use crate::adapters::{self, DiscoveredContent};
use crate::model::{AgentId, ItemKind, Scope};

pub struct DiscoverySummary {
    pub agent: AgentId,
    pub kind: ItemKind,
    pub scope: Scope,
    pub found: Vec<DiscoveredContent>,
}

/// Read-only sweep across every registered agent's supported scopes/kinds.
/// Used by `agents discover` (import candidates) and `status` (drift/presence
/// display) — never writes anything.
pub fn discover_all(project_root: &Path) -> Vec<DiscoverySummary> {
    let mut out = Vec::new();
    for agent in adapters::registry() {
        for &scope in agent.supported_scopes() {
            for &kind in agent.supported_kinds() {
                let found = agent.discover_existing(kind, scope, project_root);
                if !found.is_empty() {
                    out.push(DiscoverySummary {
                        agent: agent.id(),
                        kind,
                        scope,
                        found,
                    });
                }
            }
        }
    }
    out
}
