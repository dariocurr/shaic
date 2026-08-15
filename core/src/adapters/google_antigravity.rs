use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::discover_directory;
use super::{Agent, DiscoveredContent, RenderedFile};

/// Convention unconfirmed. Discover-only: shaic can show what's there for
/// visibility, but never writes into this agent's directory until the real
/// on-disk format is confirmed.
pub struct Antigravity;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Rule];

impl Agent for Antigravity {
    fn id(&self) -> AgentId {
        AgentId::Antigravity
    }

    fn display_name(&self) -> &'static str {
        "Google Antigravity (experimental, read-only)"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, _scope: Scope, project_root: &Path) -> Option<PathBuf> {
        // Project-scope only, so always resolvable — no home directory
        // involved.
        Some(project_root.join(".antigravity"))
    }

    fn render(
        &self,
        _kind: ItemKind,
        _items: &[Item],
        _scope: Scope,
        _existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        Vec::new()
    }

    fn discover_existing(
        &self,
        _kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent> {
        let Some(root) = self.root(scope, project_root) else {
            return Vec::new();
        };
        discover_directory(&root.join("rules"), scope, "md")
    }

    fn experimental_read_only(&self) -> bool {
        true
    }
}
