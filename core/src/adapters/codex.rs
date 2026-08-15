use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    SKILL_FILE_NAME, discover_single_file, discover_skill_files, format_skill, heading_section,
    reconcile_canonical_files, reconcile_heading_sections, render_as_directory,
    render_as_single_file,
};
use super::{Agent, DiscoveredContent, McpConfigFormat, McpTarget, RenderedFile};

pub struct Codex;

const SCOPES: &[Scope] = &[Scope::Global, Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Skill, ItemKind::Rule];

/// Codex's skill directory, relative to `root()` — asymmetric between scopes
/// because `root()` itself already points at `~/.codex` for Global (so
/// skills sit directly under it) but stays the bare project root for Project
/// (so `AGENTS.md` lands at the project root, not inside `.codex/`) — skills
/// still need the `.codex/` prefix in that case. Matches Codex CLI's actual
/// layout: `~/.codex/skills/` personal, `.codex/skills/` per-project.
fn skills_dir(scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => PathBuf::from("skills"),
        Scope::Project => PathBuf::from(".codex/skills"),
    }
}

impl Agent for Codex {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn display_name(&self) -> &'static str {
        "OpenAI Codex CLI"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, scope: Scope, project_root: &Path) -> Option<PathBuf> {
        match scope {
            // Global support is a partial exception (Codex's real global
            // mechanism is `~/.codex/config.toml`, not confirmed to be a
            // parallel AGENTS.md) — treated as best-effort, same caveat as
            // Antigravity, to be confirmed during implementation. `?`, never a
            // `.` fallback: with no resolvable home directory the global scope
            // is skipped rather than dropping a `.codex/` tree into the
            // current working directory.
            Scope::Global => Some(crate::platform::home_dir()?.join(".codex")),
            Scope::Project => Some(project_root.to_path_buf()),
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
                skills_dir(scope),
                scope,
                items,
                |item| format!("{}/{SKILL_FILE_NAME}", item.name()),
                format_skill,
            ),
            ItemKind::Rule => {
                render_as_single_file(PathBuf::from("AGENTS.md"), scope, items, |item| {
                    heading_section(item, "##")
                })
            }
            // Not in `supported_kinds()`, never actually dispatched here —
            // only present because `ItemKind` match arms must be exhaustive.
            ItemKind::Command => Vec::new(),
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
            // Only `SKILL.md`/flat `<name>.md`, same reasoning as Claude Code:
            // a skill's supporting Markdown must not become its own item.
            ItemKind::Skill => discover_skill_files(&root.join(skills_dir(scope)), scope),
            ItemKind::Rule => discover_single_file(&root.join("AGENTS.md"), scope),
            ItemKind::Command => Vec::new(),
        }
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        match kind {
            ItemKind::Skill => reconcile_canonical_files(self, kind, scope, project_root),
            ItemKind::Rule => reconcile_heading_sections(self, kind, scope, project_root, "##"),
            ItemKind::Command => Vec::new(),
        }
    }

    fn mcp_target(&self, scope: Scope, project_root: &Path) -> Option<McpTarget> {
        let path = match scope {
            Scope::Global => crate::platform::home_dir()?
                .join(".codex")
                .join("config.toml"),
            Scope::Project => project_root.join(".codex").join("config.toml"),
        };
        Some(McpTarget {
            path,
            format: McpConfigFormat::TomlTables {
                table_prefix: "mcp_servers",
            },
        })
    }
}
