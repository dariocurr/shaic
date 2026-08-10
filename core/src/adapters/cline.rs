use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    discover_directory, discover_single_file, heading_section, item_from_heading_file,
    md_file_name, reconcile_per_file_or_combined, render_as_directory, render_as_single_file,
};
use super::{Agent, DiscoveredContent, RenderedFile};

pub struct Cline;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Skill, ItemKind::Rule];

impl Agent for Cline {
    fn id(&self) -> AgentId {
        AgentId::Cline
    }

    fn display_name(&self) -> &'static str {
        "Cline"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, _scope: Scope, project_root: &Path) -> PathBuf {
        project_root.to_path_buf()
    }

    fn render(
        &self,
        _kind: ItemKind,
        items: &[Item],
        scope: Scope,
        existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        match existing_form {
            Some(ContentForm::SingleFile) => {
                render_as_single_file(PathBuf::from(".clinerules"), scope, items, |item| {
                    heading_section(item, "#")
                })
            }
            _ => render_as_directory(
                PathBuf::from(".clinerules"),
                scope,
                items,
                md_file_name,
                |item| heading_section(item, "#"),
            ),
        }
    }

    fn discover_existing(
        &self,
        _kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent> {
        let root = self.root(scope, project_root);
        let mut found = discover_directory(&root.join(".clinerules"), scope, "md");
        found.extend(discover_single_file(&root.join(".clinerules"), scope));
        found
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        // `render`/`discover_existing` ignore `kind` entirely — Skill and
        // Rule share the exact same `.clinerules` location and format, so
        // there's no way to tell which was meant on the way back. Only
        // reverse for Rule, same reasoning as Cursor/Windsurf.
        if kind != ItemKind::Rule {
            return Vec::new();
        }
        reconcile_per_file_or_combined(self, kind, scope, project_root, "#", |discovered| {
            item_from_heading_file(kind, scope, "#", discovered)
        })
    }
}
