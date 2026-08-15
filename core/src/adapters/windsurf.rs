use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    DualForm, discover_directory, discover_dual_form, file_stem_name, heading_section,
    item_from_heading_file, md_file_name, reconcile_described_files, reconcile_dual_form,
    render_as_directory, render_dual_form, with_description,
};
use super::{Agent, DiscoveredContent, McpConfigFormat, McpTarget, RenderedFile};

pub struct Windsurf;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &ItemKind::ALL;

/// Windsurf's two rule shapes: `.windsurf/rules/<name>.md` (current) and the
/// legacy single `.windsurfrules` file. Unlike Cursor's `.mdc`, each per-item
/// file is plain Markdown with a leading `# name` heading, so the same
/// `heading` is used for both forms.
fn dual_form() -> DualForm {
    DualForm {
        directory: PathBuf::from(".windsurf").join("rules"),
        extension: "md",
        legacy_file: PathBuf::from(".windsurfrules"),
        heading: "#",
    }
}

/// Commands are a separate, single-shaped location (no legacy equivalent).
fn workflows_dir() -> PathBuf {
    PathBuf::from(".windsurf").join("workflows")
}

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

    fn root(&self, _scope: Scope, project_root: &Path) -> Option<PathBuf> {
        Some(project_root.to_path_buf())
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
                workflows_dir(),
                scope,
                items,
                md_file_name,
                with_description,
            );
        }
        let form = dual_form();
        render_dual_form(&form, items, scope, existing_form, md_file_name, |item| {
            heading_section(item, form.heading)
        })
    }

    fn discover_existing(
        &self,
        kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent> {
        let Some(root) = self.root(scope, project_root) else {
            return Vec::new();
        };
        if kind == ItemKind::Command {
            return discover_directory(&root.join(workflows_dir()), scope, "md");
        }
        discover_dual_form(&dual_form(), &root, scope)
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        if kind == ItemKind::Command {
            return reconcile_described_files(self, kind, scope, project_root, file_stem_name);
        }
        // Rule-only, because `render` puts Skill in the same files with the
        // same format. See `reconcile_dual_form`.
        let form = dual_form();
        reconcile_dual_form(self, kind, scope, project_root, &form, |discovered| {
            item_from_heading_file(kind, scope, form.heading, discovered)
        })
    }

    fn mcp_target(&self, scope: Scope, _project_root: &Path) -> Option<McpTarget> {
        match scope {
            // Dedicated, MCP-only file — safe to merge into. No confirmed
            // project-scope MCP config for Windsurf, so Project stays
            // unsupported here. No fallback to `.` if the home directory
            // can't be determined — better to report "unsupported" than to
            // silently write into the current working directory.
            Scope::Global => Some(McpTarget {
                path: crate::platform::home_dir()?
                    .join(".codeium")
                    .join("windsurf")
                    .join("mcp_config.json"),
                format: McpConfigFormat::Json {
                    servers_key: "mcpServers",
                },
            }),
            Scope::Project => None,
        }
    }
}
