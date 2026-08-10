use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Frontmatter, Item, ItemKind, Scope};

use super::common::{
    discover_directory, discover_single_file, discover_unowned, heading_section,
    reconcile_described_files, reconcile_heading_sections, reconciled_frontmatter,
    render_as_directory, render_as_single_file, split_frontmatter_block, split_globs,
    with_description,
};
use super::{Agent, DiscoveredContent, McpTarget, RenderedFile};

pub struct Copilot;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &ItemKind::ALL;

impl Agent for Copilot {
    fn id(&self) -> AgentId {
        AgentId::Copilot
    }

    fn display_name(&self) -> &'static str {
        "GitHub Copilot"
    }

    fn supported_scopes(&self) -> &'static [Scope] {
        SCOPES
    }

    fn supported_kinds(&self) -> &'static [ItemKind] {
        KINDS
    }

    fn root(&self, _scope: Scope, project_root: &Path) -> PathBuf {
        project_root.join(".github")
    }

    fn render(
        &self,
        kind: ItemKind,
        items: &[Item],
        scope: Scope,
        _existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile> {
        match kind {
            ItemKind::Rule => render_as_single_file(
                PathBuf::from("copilot-instructions.md"),
                scope,
                items,
                |item| heading_section(item, "##"),
            ),
            ItemKind::Skill => render_as_directory(
                PathBuf::from("instructions"),
                scope,
                items,
                |item| format!("{}.instructions.md", item.name()),
                format_scoped_instruction,
            ),
            ItemKind::Command => render_as_directory(
                PathBuf::from("prompts"),
                scope,
                items,
                |item| format!("{}.prompt.md", item.name()),
                with_description,
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
        match kind {
            ItemKind::Rule => discover_single_file(&root.join("copilot-instructions.md"), scope),
            ItemKind::Skill => discover_directory(&root.join("instructions"), scope, "md"),
            ItemKind::Command => discover_directory(&root.join("prompts"), scope, "md"),
        }
    }

    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        match kind {
            ItemKind::Rule => reconcile_heading_sections(self, kind, scope, project_root, "##"),
            // `format_scoped_instruction` never wrote a description at all —
            // only `applyTo` and the body — so there's none to recover
            // either; `applyTo: "**"` round-trips back to an empty
            // `applies_to`, the same "matches everything" default the
            // forward direction uses.
            ItemKind::Skill => discover_unowned(self, kind, scope, project_root)
                .iter()
                .filter_map(|discovered| {
                    let name = strip_file_suffix(&discovered.source_path, ".instructions.md")?;
                    let (fm, body) = split_frontmatter_block(&discovered.raw)?;
                    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(fm).ok()?;
                    let apply_to = value["applyTo"].as_str().unwrap_or("**");
                    let applies_to = if apply_to == "**" {
                        Vec::new()
                    } else {
                        split_globs(apply_to)
                    };
                    Item::new(
                        kind,
                        Frontmatter {
                            applies_to,
                            ..reconciled_frontmatter(name, scope)
                        },
                        body.trim().to_string(),
                    )
                    .ok()
                })
                .collect(),
            ItemKind::Command => {
                reconcile_described_files(self, kind, scope, project_root, |path| {
                    strip_file_suffix(path, ".prompt.md")
                })
            }
        }
    }

    fn mcp_target(&self, scope: Scope, project_root: &Path) -> Option<McpTarget> {
        match scope {
            // `.vscode/mcp.json` is dedicated to MCP servers and meant to be
            // committed — safe to merge into. Note the top-level key is
            // `servers`, not `mcpServers` like every other agent here.
            Scope::Project => Some(McpTarget {
                path: project_root.join(".vscode").join("mcp.json"),
                servers_key: "servers",
            }),
            // User/global config lives at a VS Code profile-dependent path
            // with no single documented location — unsupported here.
            Scope::Global => None,
        }
    }
}

fn format_scoped_instruction(item: &Item) -> String {
    let apply_to = if item.frontmatter.applies_to.is_empty() {
        "**".to_string()
    } else {
        item.frontmatter.applies_to.join(",")
    };
    format!("---\napplyTo: \"{apply_to}\"\n---\n\n{}", item.body.trim())
}

/// `Path::file_stem()` only strips the last `.md`, leaving e.g.
/// `foo.instructions` for `foo.instructions.md` — Copilot's Skill/Command
/// filenames carry a second, meaningful extension segment that needs
/// stripping explicitly to recover the real item name.
fn strip_file_suffix(path: &Path, suffix: &str) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(suffix)
        .map(String::from)
}
