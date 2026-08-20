use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

use super::common::{
    SKILL_FILE_NAME, discover_directory, discover_single_file, discover_skill_files,
    file_stem_name, format_skill, heading_section, md_file_name, reconcile_canonical_files,
    reconcile_described_files, reconcile_heading_sections, render_as_directory,
    render_as_single_file, with_description,
};
use super::{Agent, DiscoveredContent, McpConfigFormat, McpTarget, RenderedFile};

pub struct OpenCode;

const SCOPES: &[Scope] = &[Scope::Global, Scope::Project];
const KINDS: &[ItemKind] = &ItemKind::ALL;

/// OpenCode's documented global root is always XDG-style
/// (`$XDG_CONFIG_HOME/opencode` or `~/.config/opencode`), never macOS
/// Application Support — matching https://opencode.ai/docs/config/.
fn global_root() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::platform::home_dir().map(|h| h.join(".config")))?;
    Some(config.join("opencode"))
}

/// Skills/commands sit under `root()` for Global, but need the `.opencode/`
/// prefix for Project — `root()` is the bare project so `AGENTS.md` and
/// `opencode.json` land at the project root (same asymmetry as Codex).
fn skills_dir(scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => PathBuf::from("skills"),
        Scope::Project => PathBuf::from(".opencode/skills"),
    }
}

fn commands_dir(scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => PathBuf::from("commands"),
        Scope::Project => PathBuf::from(".opencode/commands"),
    }
}

impl Agent for OpenCode {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, scope: Scope, project_root: &Path) -> Option<PathBuf> {
        match scope {
            // `?`, never a `.` fallback: with no resolvable home/config dir the
            // global scope is skipped rather than writing into cwd.
            Scope::Global => global_root(),
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
            ItemKind::Command => render_as_directory(
                commands_dir(scope),
                scope,
                items,
                md_file_name,
                with_description,
            ),
            // Project `AGENTS.md` is shared with Codex (one managed region).
            // `plan::item_targets_agent` writes the union of Codex+OpenCode
            // rules so a multi-agent sync does not clobber either side.
            ItemKind::Rule => {
                render_as_single_file(PathBuf::from("AGENTS.md"), scope, items, |item| {
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
            ItemKind::Skill => discover_skill_files(&root.join(skills_dir(scope)), scope),
            ItemKind::Command => discover_directory(&root.join(commands_dir(scope)), scope, "md"),
            ItemKind::Rule => discover_single_file(&root.join("AGENTS.md"), scope),
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
        // Shared settings file (model, permissions, …). Merge only rewrites the
        // top-level `mcp` key — same blast-radius caution as Claude's global
        // `~/.claude.json`.
        let path = match scope {
            Scope::Global => global_root()?.join("opencode.json"),
            Scope::Project => project_root.join("opencode.json"),
        };
        Some(McpTarget {
            path,
            format: McpConfigFormat::OpenCodeJson { servers_key: "mcp" },
        })
    }
}
