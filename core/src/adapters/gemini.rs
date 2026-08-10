use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    discover_single_file, heading_section, reconcile_heading_sections, render_as_single_file,
};
use super::{Agent, DiscoveredContent, RenderedFile};

pub struct Gemini;

const SCOPES: &[Scope] = &[Scope::Global, Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Rule];

impl Agent for Gemini {
    fn id(&self) -> AgentId {
        AgentId::Gemini
    }

    fn display_name(&self) -> &'static str {
        "Google Gemini CLI"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, scope: Scope, project_root: &Path) -> PathBuf {
        match scope {
            Scope::Global => dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".gemini"),
            Scope::Project => project_root.to_path_buf(),
        }
    }

    fn render(
        &self,
        _kind: ItemKind,
        items: &[Item],
        scope: Scope,
        _existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        render_as_single_file(PathBuf::from("GEMINI.md"), scope, items, |item| {
            heading_section(item, "##")
        })
    }

    fn discover_existing(
        &self,
        _kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent> {
        discover_single_file(&self.root(scope, project_root).join("GEMINI.md"), scope)
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        reconcile_heading_sections(self, kind, scope, project_root, "##")
    }
}
