use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    SKILL_FILE_NAME, discover_directory, discover_single_file, discover_skill_files,
    file_stem_name, format_skill, heading_section, md_file_name, reconcile_canonical_files,
    reconcile_described_files, reconcile_heading_sections, render_as_directory,
    render_as_single_file, with_description,
};
use super::{Agent, DiscoveredContent, McpConfigFormat, McpTarget, RenderedFile};

pub struct ClaudeCode;

const SCOPES: &[Scope] = &[Scope::Global, Scope::Project];
const KINDS: &[ItemKind] = &ItemKind::ALL;

impl Agent for ClaudeCode {
    fn id(&self) -> AgentId {
        AgentId::ClaudeCode
    }

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, scope: Scope, project_root: &Path) -> Option<PathBuf> {
        match scope {
            // `?`, never a `.` fallback: with no resolvable home directory the
            // right answer is "skip the global scope", not "write a `.claude/`
            // tree into whatever directory the user is standing in".
            Scope::Global => Some(crate::platform::home_dir()?.join(".claude")),
            Scope::Project => Some(project_root.join(".claude")),
        }
    }

    fn render(
        &self,
        kind: ItemKind,
        items: &[Item],
        scope: Scope,
        _existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        match kind {
            ItemKind::Skill => render_as_directory(
                PathBuf::from("skills"),
                scope,
                items,
                |item| format!("{}/{SKILL_FILE_NAME}", item.name()),
                format_skill,
            ),
            ItemKind::Command => render_as_directory(
                PathBuf::from("commands"),
                scope,
                items,
                md_file_name,
                with_description,
            ),
            ItemKind::Rule => {
                render_as_single_file(PathBuf::from("CLAUDE.md"), scope, items, |item| {
                    heading_section(item, "##")
                })
            }
        }
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
        match kind {
            // Only `SKILL.md` (at any nesting depth) and flat `<name>.md`,
            // not every `*.md` under `skills/` — a skill directory's own
            // supporting docs are payload, not separate items.
            ItemKind::Skill => discover_skill_files(&root.join("skills"), scope),
            ItemKind::Command => discover_directory(&root.join("commands"), scope, "md"),
            ItemKind::Rule => discover_single_file(&root.join("CLAUDE.md"), scope),
        }
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        match kind {
            ItemKind::Skill => reconcile_canonical_files(self, kind, scope, project_root),
            ItemKind::Command => {
                reconcile_described_files(self, kind, scope, project_root, file_stem_name)
            }
            ItemKind::Rule => reconcile_heading_sections(self, kind, scope, project_root, "##"),
        }
    }

    fn mcp_target(&self, scope: Scope, project_root: &Path) -> Option<McpTarget> {
        match scope {
            // `.mcp.json` is dedicated to MCP servers and meant to be
            // committed to the project — safe to merge into.
            Scope::Project => Some(McpTarget {
                path: project_root.join(".mcp.json"),
                format: McpConfigFormat::Json {
                    servers_key: "mcpServers",
                },
            }),
            // Global MCP servers live inside `~/.claude.json`, a large
            // shared state file (project list, per-project approval flags,
            // auth, ...). Merging is still safe here — `write_managed_object`
            // rewrites only the named top-level key and round-trips every
            // other key untouched (see
            // `write_managed_object_preserves_unrelated_top_level_keys`) — but
            // a bug in that path has a much bigger blast radius than any
            // other MCP target in this crate, since it's the same file that
            // holds this machine's Claude Code auth/session state. No
            // fallback to `.` if the home directory can't be determined —
            // better to report "unsupported" than to guess.
            Scope::Global => Some(McpTarget {
                path: crate::platform::home_dir()?.join(".claude.json"),
                format: McpConfigFormat::Json {
                    servers_key: "mcpServers",
                },
            }),
        }
    }
}
