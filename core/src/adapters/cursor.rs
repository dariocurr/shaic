use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Frontmatter, Item, ItemKind, Scope};

use super::common::{
    discover_directory, discover_single_file, file_stem_name, heading_section,
    reconcile_per_file_or_combined, reconciled_frontmatter, render_as_directory,
    render_as_single_file, split_frontmatter_block, split_globs,
};
use super::{Agent, DiscoveredContent, McpTarget, RenderedFile};

pub struct Cursor;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Skill, ItemKind::Rule];

impl Agent for Cursor {
    fn id(&self) -> AgentId {
        AgentId::Cursor
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
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
                render_as_single_file(PathBuf::from(".cursorrules"), scope, items, |item| {
                    heading_section(item, "#")
                })
            }
            _ => render_as_directory(
                PathBuf::from(".cursor").join("rules"),
                scope,
                items,
                |item| format!("{}.mdc", item.name()),
                format_mdc,
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
        let mut found = discover_directory(&root.join(".cursor").join("rules"), scope, "mdc");
        found.extend(discover_single_file(&root.join(".cursorrules"), scope));
        found
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        // Cursor has no separate "skill" concept — `render`/`discover_existing`
        // treat Skill and Rule identically (both are just ".mdc" rules), so
        // there's no way to tell which of Cursor's rules were meant as which
        // kind on the way back. Only reverse for Rule, the kind Cursor
        // actually calls this; treating Skill as reversible too would import
        // every rule twice under two different names.
        if kind != ItemKind::Rule {
            return Vec::new();
        }
        reconcile_per_file_or_combined(self, kind, scope, project_root, "#", |discovered| {
            item_from_mdc(kind, scope, discovered)
        })
    }

    fn mcp_target(&self, scope: Scope, project_root: &Path) -> Option<McpTarget> {
        // Both scopes use a dedicated, MCP-only file — safe to merge into. No
        // fallback to `.` if the home directory can't be determined — better
        // to report "unsupported" than to silently write into the current
        // working directory.
        let path = match scope {
            Scope::Project => project_root.join(".cursor").join("mcp.json"),
            Scope::Global => dirs::home_dir()?.join(".cursor").join("mcp.json"),
        };
        Some(McpTarget {
            path,
            servers_key: "mcpServers",
        })
    }
}

/// Reverse of `format_mdc`. The name comes from the filename, not the
/// frontmatter — `format_mdc` never wrote one there.
fn item_from_mdc(kind: ItemKind, scope: Scope, discovered: &DiscoveredContent) -> Option<Item> {
    let name = file_stem_name(&discovered.source_path)?;
    let (fm, body) = split_frontmatter_block(&discovered.raw)?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(fm).ok()?;
    let frontmatter = Frontmatter {
        description: value["description"].as_str().unwrap_or("").to_string(),
        applies_to: split_globs(value["globs"].as_str().unwrap_or("")),
        ..reconciled_frontmatter(name, scope)
    };
    Item::new(kind, frontmatter, body.trim().to_string()).ok()
}

fn format_mdc(item: &Item) -> String {
    let globs = item.frontmatter.applies_to.join(",");
    format!(
        "---\ndescription: {}\nglobs: {}\nalwaysApply: {}\n---\n\n{}",
        item.frontmatter.description,
        globs,
        globs.is_empty(),
        item.body.trim()
    )
}
