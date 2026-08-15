use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    DualForm, discover_dual_form, heading_section, item_from_heading_file, md_file_name,
    reconcile_dual_form, render_dual_form,
};
use super::{Agent, DiscoveredContent, RenderedFile};

pub struct Cline;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Skill, ItemKind::Rule];

/// Cline reuses the same name for both shapes: `.clinerules/` as a directory
/// of per-item `.md` files (current), or `.clinerules` as a single combined
/// file (legacy). A path can only be one of the two at a time, so
/// `discover_dual_form`'s "directory first, else the file" order resolves it
/// without any extra special-casing.
fn dual_form() -> DualForm {
    DualForm {
        directory: PathBuf::from(".clinerules"),
        extension: "md",
        legacy_file: PathBuf::from(".clinerules"),
        heading: "#",
    }
}

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

    fn root(&self, _scope: Scope, project_root: &Path) -> Option<PathBuf> {
        Some(project_root.to_path_buf())
    }

    fn render(
        &self,
        _kind: ItemKind,
        items: &[Item],
        scope: Scope,
        existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        let form = dual_form();
        render_dual_form(&form, items, scope, existing_form, md_file_name, |item| {
            heading_section(item, form.heading)
        })
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
        discover_dual_form(&dual_form(), &root, scope)
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        // Rule-only: `render`/`discover_existing` ignore `kind` entirely here,
        // so Skill and Rule are indistinguishable on the way back. See
        // `reconcile_dual_form`.
        let form = dual_form();
        reconcile_dual_form(self, kind, scope, project_root, &form, |discovered| {
            item_from_heading_file(kind, scope, form.heading, discovered)
        })
    }
}
