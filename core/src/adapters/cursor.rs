use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::{AgentId, ContentForm, Frontmatter, Item, ItemKind, Scope};

use super::common::{
    DualForm, discover_dual_form, file_stem_name, frontmatter_document, frontmatter_str,
    parse_frontmatter_value, reconcile_dual_form, reconciled_frontmatter, render_dual_form,
    split_frontmatter_block, split_globs,
};
use super::{Agent, DiscoveredContent, McpConfigFormat, McpTarget, RenderedFile};

pub struct Cursor;

const SCOPES: &[Scope] = &[Scope::Project];
const KINDS: &[ItemKind] = &[ItemKind::Skill, ItemKind::Rule];

/// Cursor's two shapes: `.cursor/rules/<name>.mdc` (current) and the legacy
/// single `.cursorrules` file. `discover_dual_form` picks exactly one of them
/// as the source of truth for both directions.
fn dual_form() -> DualForm {
    DualForm {
        directory: PathBuf::from(".cursor").join("rules"),
        extension: "mdc",
        legacy_file: PathBuf::from(".cursorrules"),
        heading: "#",
    }
}

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
        render_dual_form(
            &dual_form(),
            items,
            scope,
            existing_form,
            // `.mdc`, Cursor's own extension, and unlike Windsurf/Cline the
            // per-item file carries real frontmatter rather than a heading.
            |item| format!("{}.mdc", item.name()),
            format_mdc,
        )
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
        // Rule-only, because Cursor has no separate "skill" concept: both
        // kinds render to identical `.mdc` rules. See `reconcile_dual_form`.
        reconcile_dual_form(
            self,
            kind,
            scope,
            project_root,
            &dual_form(),
            |discovered| item_from_mdc(kind, scope, discovered),
        )
    }

    fn mcp_target(&self, scope: Scope, project_root: &Path) -> Option<McpTarget> {
        // Both scopes use a dedicated, MCP-only file — safe to merge into. No
        // fallback to `.` if the home directory can't be determined — better
        // to report "unsupported" than to silently write into the current
        // working directory.
        let path = match scope {
            Scope::Project => project_root.join(".cursor").join("mcp.json"),
            Scope::Global => crate::platform::home_dir()?
                .join(".cursor")
                .join("mcp.json"),
        };
        Some(McpTarget {
            path,
            format: McpConfigFormat::Json {
                servers_key: "mcpServers",
            },
        })
    }
}

/// The exact `.mdc` frontmatter Cursor reads. A struct, not a `format!`
/// template: the field order here *is* the on-disk order, and an item's
/// `description` can no longer smuggle in an extra key (`alwaysApply: true`
/// would have forced the item into every prompt) — see
/// `common::frontmatter_document`.
#[derive(Serialize)]
struct MdcFrontmatter<'a> {
    description: &'a str,
    globs: &'a str,
    #[serde(rename = "alwaysApply")]
    always_apply: bool,
}

fn format_mdc(item: &Item) -> String {
    let globs = item.frontmatter.applies_to.join(",");
    frontmatter_document(
        &MdcFrontmatter {
            description: &item.frontmatter.description,
            globs: &globs,
            // No globs at all means "no file filter", which Cursor spells as
            // "always apply".
            always_apply: globs.is_empty(),
        },
        &item.body,
    )
}

/// Reverse of `format_mdc`. The name comes from the filename, not the
/// frontmatter — `format_mdc` never wrote one there. A missing or
/// non-string `description`/`globs` reads as empty (which `reconcile_items`
/// treats as "inherit whatever the store had") rather than being coerced
/// into a guess.
fn item_from_mdc(kind: ItemKind, scope: Scope, discovered: &DiscoveredContent) -> Option<Item> {
    let name = file_stem_name(&discovered.source_path)?;
    let (fm, body) = split_frontmatter_block(&discovered.raw)?;
    let value = parse_frontmatter_value(fm)?;
    let frontmatter = Frontmatter {
        description: frontmatter_str(&value, "description")
            .unwrap_or_default()
            .to_string(),
        applies_to: split_globs(frontmatter_str(&value, "globs").unwrap_or_default()),
        ..reconciled_frontmatter(name, scope)
    };
    Item::new(kind, frontmatter, body.trim().to_string()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, description: &str, applies_to: Vec<String>, body: &str) -> Item {
        Item::new(
            ItemKind::Rule,
            Frontmatter {
                name: name.to_string(),
                description: description.to_string(),
                applies_to,
                tags: vec![],
                scope: vec![Scope::Project],
                agents: AgentId::ALL.to_vec(),
            },
            body.to_string(),
        )
        .expect("test item name is valid")
    }

    fn round_trip(item: &Item) -> Item {
        let rendered = format_mdc(item);
        let discovered = DiscoveredContent {
            source_path: PathBuf::from(".cursor/rules").join(format!("{}.mdc", item.name())),
            scope: Scope::Project,
            raw: rendered,
            form: ContentForm::Directory,
        };
        item_from_mdc(ItemKind::Rule, Scope::Project, &discovered).expect("round-trips")
    }

    #[test]
    fn mdc_round_trips_hostile_descriptions_without_injecting_keys() {
        for description in [
            "legit\nalwaysApply: true\nbogus: ",
            "has: a colon",
            "*leading star",
            "#leading hash",
            "a \"double quote\" inside",
            "before\n---\nafter",
        ] {
            let item = rule("no-any", description, vec![], "The body.");
            let rendered = format_mdc(&item);
            let (fm, _) = split_frontmatter_block(&rendered).expect("frontmatter block");
            let value = parse_frontmatter_value(fm).expect("parses as a mapping");
            let keys: Vec<String> = value
                .as_mapping()
                .expect("mapping")
                .keys()
                .filter_map(|k| k.as_str().map(String::from))
                .collect();
            assert_eq!(
                keys,
                vec![
                    "description".to_string(),
                    "globs".to_string(),
                    "alwaysApply".to_string()
                ],
                "a description must never add a key: {rendered}"
            );
            assert_eq!(
                value.get("alwaysApply").and_then(|v| v.as_bool()),
                Some(true),
                "an item must not be able to set alwaysApply itself: {rendered}"
            );

            let back = round_trip(&item);
            assert_eq!(back.frontmatter.description, description);
            assert_eq!(back.body, "The body.");
        }
    }

    #[test]
    fn mdc_round_trips_brace_globs_intact() {
        let applies_to = vec!["{src,dist}/**/*.ts".to_string(), "docs/*.md".to_string()];
        let item = rule("scoped", "d", applies_to.clone(), "Body.");
        let back = round_trip(&item);
        assert_eq!(
            back.frontmatter.applies_to, applies_to,
            "a brace alternation must not be torn apart by the comma split"
        );
    }
}
