use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    discover_directory, discover_single_file, file_stem_name, heading_section,
    item_from_heading_file, md_file_name, reconcile_described_files,
    reconcile_per_file_or_combined, render_as_directory, render_as_single_file, with_description,
};
use super::{Agent, DiscoveredContent, McpTarget, RenderedFile};

pub struct Windsurf;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &ItemKind::ALL;

impl Agent for Windsurf {
    fn id(&self) -> AgentId {
        AgentId::Windsurf
    }

    fn display_name(&self) -> &'static str {
        "Windsurf"
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
        kind: ItemKind,
        items: &[Item],
        scope: Scope,
        existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        if kind == ItemKind::Command {
            return render_as_directory(
                PathBuf::from(".windsurf").join("workflows"),
                scope,
                items,
                md_file_name,
                with_description,
            );
        }
        match existing_form {
            Some(ContentForm::SingleFile) => {
                render_as_single_file(PathBuf::from(".windsurfrules"), scope, items, |item| {
                    heading_section(item, "#")
                })
            }
            _ => render_as_directory(
                PathBuf::from(".windsurf").join("rules"),
                scope,
                items,
                md_file_name,
                |item| heading_section(item, "#"),
            ),
        }
    }

    fn discover_existing(
        &self,
        kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent> {
        let root = self.root(scope, project_root);
        if kind == ItemKind::Command {
            return discover_directory(&root.join(".windsurf").join("workflows"), scope, "md");
        }
        let mut found = discover_directory(&root.join(".windsurf").join("rules"), scope, "md");
        found.extend(discover_single_file(&root.join(".windsurfrules"), scope));
        found
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        match kind {
            // `render`/`discover_existing` treat Skill identically to Rule
            // here (same directory, same format) — no way to tell which of
            // Windsurf's rules were meant as which kind on the way back.
            // Only reverse for Rule, same reasoning as Cursor.
            ItemKind::Skill => Vec::new(),
            ItemKind::Command => {
                reconcile_described_files(self, kind, scope, project_root, file_stem_name)
            }
            ItemKind::Rule => {
                reconcile_per_file_or_combined(self, kind, scope, project_root, "#", |discovered| {
                    item_from_heading_file(kind, scope, "#", discovered)
                })
            }
        }
    }

    fn mcp_target(&self, scope: Scope, _project_root: &Path) -> Option<McpTarget> {
        match scope {
            // Dedicated, MCP-only file — safe to merge into. No confirmed
            // project-scope MCP config for Windsurf, so Project stays
            // unsupported here. No fallback to `.` if the home directory
            // can't be determined — better to report "unsupported" than to
            // silently write into the current working directory.
            Scope::Global => Some(McpTarget {
                path: dirs::home_dir()?
                    .join(".codeium")
                    .join("windsurf")
                    .join("mcp_config.json"),
                servers_key: "mcpServers",
            }),
            Scope::Project => None,
        }
    }
}
