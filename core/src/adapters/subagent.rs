//! Shared Subagent render / discover / reconcile for Claude, OpenCode, Cursor.
//!
//! Canonical store fields: name, description, body, `tools`, `native.<agent>`.
//! Provider-only keys park under `native`; tools map where the provider can
//! express them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_yaml_ng::Value;

use crate::model::{AgentId, Frontmatter, Item, ItemKind, Scope};

use super::common::{
    discover_directory, discover_unowned, file_stem_name, frontmatter_document, frontmatter_str,
    md_file_name, parse_frontmatter_value, render_as_directory, split_frontmatter_block,
};
use super::{Agent, DiscoveredContent, RenderedFile};

/// Canonical portable tool names (Claude vocabulary as interchange).
pub const WRITE_TOOLS: &[&str] = &["Write", "Edit"];
pub const BASH_TOOL: &str = "Bash";
pub const WEBFETCH_TOOL: &str = "WebFetch";

/// Frontmatter keys that are portable / identity — everything else goes into
/// `native.<agent-id>` on import.
const PORTABLE_KEYS: &[&str] = &["name", "description", "tools"];

fn tools_has_any(tools: &[String], names: &[&str]) -> bool {
    tools
        .iter()
        .any(|t| names.iter().any(|n| t.eq_ignore_ascii_case(n)))
}

fn tools_has(tools: &[String], name: &str) -> bool {
    tools.iter().any(|t| t.eq_ignore_ascii_case(name))
}

/// Cursor `readonly: true` when the allowlist has no write/bash tools.
/// Empty tools is treated as unknown/locked → readonly (never escalate).
pub fn cursor_readonly(tools: &[String]) -> bool {
    tools.is_empty() || (!tools_has_any(tools, WRITE_TOOLS) && !tools_has(tools, BASH_TOOL))
}

/// Parse a comma-or-list tools field from YAML into Claude-style names.
fn parse_tools_value(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Park every non-portable frontmatter key under `native.<agent>`.
fn native_overlay(mapping: &serde_yaml_ng::Mapping, agent: AgentId) -> BTreeMap<String, Value> {
    let mut overlay = serde_yaml_ng::Mapping::new();
    for (k, v) in mapping {
        let Some(key) = k.as_str() else {
            continue;
        };
        if PORTABLE_KEYS.contains(&key) {
            continue;
        }
        overlay.insert(Value::String(key.to_string()), v.clone());
    }
    if overlay.is_empty() {
        return BTreeMap::new();
    }
    let mut native = BTreeMap::new();
    native.insert(agent.as_str().to_string(), Value::Mapping(overlay));
    native
}

fn merge_native_into_map(
    map: &mut serde_yaml_ng::Mapping,
    native: &BTreeMap<String, Value>,
    agent: AgentId,
) {
    let Some(Value::Mapping(overlay)) = native.get(agent.as_str()) else {
        return;
    };
    for (k, v) in overlay {
        if let Some(key) = k.as_str()
            && !PORTABLE_KEYS.contains(&key)
            && key != "readonly"
        {
            map.insert(Value::String(key.to_string()), v.clone());
        }
    }
}

fn frontmatter_from_map(map: serde_yaml_ng::Mapping, body: &str) -> String {
    let yaml = serde_yaml_ng::to_string(&Value::Mapping(map)).unwrap_or_default();
    let yaml = if yaml.ends_with('\n') {
        yaml
    } else {
        format!("{yaml}\n")
    };
    format!("---\n{yaml}---\n\n{}", body.trim())
}

pub fn format_claude(item: &Item) -> String {
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(
        Value::String("name".into()),
        Value::String(item.name().to_string()),
    );
    map.insert(
        Value::String("description".into()),
        Value::String(item.frontmatter.description.clone()),
    );
    if !item.frontmatter.tools.is_empty() {
        map.insert(
            Value::String("tools".into()),
            Value::String(item.frontmatter.tools.join(", ")),
        );
    }
    merge_native_into_map(&mut map, &item.frontmatter.native, AgentId::ClaudeCode);
    frontmatter_from_map(map, &item.body)
}

/// OpenCode markdown agent: filename is id; body is system prompt.
pub fn format_opencode(item: &Item) -> String {
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(
        Value::String("description".into()),
        Value::String(item.frontmatter.description.clone()),
    );
    // Default mode for portable subagents; native overlay can override.
    map.insert(
        Value::String("mode".into()),
        Value::String("subagent".into()),
    );

    // Merge native first (mode, color, leftover globs). Then apply portable
    // tools policy last so sticky native.permission cannot widen the cage.
    // Empty tools ⇒ deny edit/bash/webfetch (never escalate to full power).
    merge_native_into_map(&mut map, &item.frontmatter.native, AgentId::OpenCode);
    map.remove(Value::String("permission".into()));
    map.remove(Value::String("permissions".into()));
    if let Some(permission) = opencode_permission_from_tools(&item.frontmatter.tools) {
        map.insert(
            Value::String("permission".into()),
            Value::Mapping(permission),
        );
    }

    frontmatter_from_map(map, &item.body)
}

fn opencode_permission_from_tools(tools: &[String]) -> Option<serde_yaml_ng::Mapping> {
    let mut permission = serde_yaml_ng::Mapping::new();
    let deny_edit = tools.is_empty() || !tools_has_any(tools, WRITE_TOOLS);
    let deny_bash = tools.is_empty() || !tools_has(tools, BASH_TOOL);
    let deny_fetch = tools.is_empty() || !tools_has(tools, WEBFETCH_TOOL);
    if deny_edit {
        permission.insert(Value::String("edit".into()), Value::String("deny".into()));
    }
    if deny_bash {
        permission.insert(Value::String("bash".into()), Value::String("deny".into()));
    }
    if deny_fetch {
        permission.insert(
            Value::String("webfetch".into()),
            Value::String("deny".into()),
        );
    }
    if permission.is_empty() {
        None
    } else {
        Some(permission)
    }
}

#[derive(Serialize)]
struct CursorAgentFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    readonly: bool,
}

pub fn format_cursor(item: &Item) -> String {
    frontmatter_document(
        &CursorAgentFrontmatter {
            name: item.name(),
            description: &item.frontmatter.description,
            readonly: cursor_readonly(&item.frontmatter.tools),
        },
        &item.body,
    )
}

fn parse_tools_from_mapping(mapping: &serde_yaml_ng::Mapping) -> Vec<String> {
    if let Some(v) = mapping.get(Value::String("tools".into())) {
        return parse_tools_value(v);
    }
    // OpenCode permission → approximate tools allowlist (inverse of emit).
    if let Some(Value::Mapping(perm)) = mapping.get(Value::String("permission".into())) {
        let mut tools = vec!["Read".into(), "Glob".into(), "Grep".into(), "List".into()];
        let edit_denied = perm
            .get(Value::String("edit".into()))
            .and_then(|v| v.as_str())
            == Some("deny");
        let bash_denied = perm
            .get(Value::String("bash".into()))
            .and_then(|v| v.as_str())
            == Some("deny");
        let fetch_denied = perm
            .get(Value::String("webfetch".into()))
            .and_then(|v| v.as_str())
            == Some("deny");
        if !edit_denied {
            tools.push("Write".into());
            tools.push("Edit".into());
        }
        if !bash_denied {
            tools.push("Bash".into());
        }
        if !fetch_denied {
            tools.push("WebFetch".into());
        }
        return tools;
    }
    // Cursor readonly → read-ish tools only.
    if mapping
        .get(Value::String("readonly".into()))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        return vec!["Read".into(), "Glob".into(), "Grep".into()];
    }
    Vec::new()
}

fn item_from_agent_file(
    kind: ItemKind,
    scope: Scope,
    agent: AgentId,
    discovered: &DiscoveredContent,
    name_from_path: bool,
) -> Option<Item> {
    let path_name = file_stem_name(&discovered.source_path)?;
    let (fm, body) = split_frontmatter_block(&discovered.raw)?;
    let value = parse_frontmatter_value(fm)?;
    let mapping = value.as_mapping()?;

    let name = if name_from_path {
        path_name
    } else {
        frontmatter_str(&value, "name")
            .map(str::to_string)
            .filter(|n| crate::model::validate_name(n).is_ok())
            .unwrap_or(path_name)
    };

    let description = frontmatter_str(&value, "description")
        .unwrap_or_default()
        .to_string();
    let tools = parse_tools_from_mapping(mapping);
    let native = native_overlay(mapping, agent);

    let frontmatter = Frontmatter {
        name,
        description,
        applies_to: Vec::new(),
        tags: Vec::new(),
        scope: vec![scope],
        agents: ItemKind::SUBAGENT_AGENTS.to_vec(),
        tools,
        native,
    };
    Item::new(kind, frontmatter, body.trim().to_string()).ok()
}

pub fn claude_agents_dir(_scope: Scope) -> PathBuf {
    PathBuf::from("agents")
}

pub fn opencode_agents_write_dir(scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => PathBuf::from("agents"),
        Scope::Project => PathBuf::from(".opencode/agents"),
    }
}

pub fn opencode_agents_discover_dirs(scope: Scope) -> [PathBuf; 2] {
    match scope {
        Scope::Global => [PathBuf::from("agents"), PathBuf::from("agent")],
        Scope::Project => [
            PathBuf::from(".opencode/agents"),
            PathBuf::from(".opencode/agent"),
        ],
    }
}

pub fn cursor_agents_dir() -> PathBuf {
    PathBuf::from(".cursor").join("agents")
}

/// Absolute path to Claude's markdown agent file for `name`, if the root exists.
pub fn claude_agent_file(scope: Scope, project_root: &Path, name: &str) -> Option<PathBuf> {
    let root = match scope {
        Scope::Global => crate::platform::home_dir()?.join(".claude"),
        Scope::Project => project_root.join(".claude"),
    };
    Some(root.join("agents").join(format!("{name}.md")))
}

/// Skip Cursor write when Claude markdown already covers this id, or when the
/// item targets Claude (this sync will write Claude).
pub fn cursor_should_skip_subagent(item: &Item, scope: Scope, project_root: &Path) -> bool {
    if item.frontmatter.agents.contains(&AgentId::ClaudeCode) {
        return true;
    }
    claude_agent_file(scope, project_root, item.name()).is_some_and(|p| p.is_file())
}

pub fn render_claude(items: &[Item], scope: Scope) -> Vec<RenderedFile> {
    render_as_directory(
        claude_agents_dir(scope),
        scope,
        items,
        md_file_name,
        format_claude,
    )
}

pub fn render_opencode(items: &[Item], scope: Scope) -> Vec<RenderedFile> {
    render_as_directory(
        opencode_agents_write_dir(scope),
        scope,
        items,
        md_file_name,
        format_opencode,
    )
}

/// Cursor render only. Skip-if-Claude lives in `plan_materialize` so omitted
/// paths drop out of `rendered_paths` and tracked shadows get deleted.
pub fn render_cursor(items: &[Item], scope: Scope) -> Vec<RenderedFile> {
    render_as_directory(
        cursor_agents_dir(),
        scope,
        items,
        md_file_name,
        format_cursor,
    )
}

fn discover_md_dir(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
    rel: PathBuf,
) -> Vec<DiscoveredContent> {
    let Some(root) = agent.root(scope, project_root) else {
        return Vec::new();
    };
    discover_directory(&root.join(rel), scope, "md")
}

pub fn discover_claude(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    discover_md_dir(agent, scope, project_root, claude_agents_dir(scope))
}

pub fn discover_opencode(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    let Some(root) = agent.root(scope, project_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in opencode_agents_discover_dirs(scope) {
        for d in discover_directory(&root.join(&dir), scope, "md") {
            let key = file_stem_name(&d.source_path).unwrap_or_default();
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            out.push(d);
        }
    }
    out
}

pub fn discover_cursor(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    discover_md_dir(agent, scope, project_root, cursor_agents_dir())
}

fn reconcile(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
    agent_id: AgentId,
    name_from_path: bool,
) -> Vec<Item> {
    discover_unowned(agent, ItemKind::Subagent, scope, project_root)
        .into_iter()
        .filter_map(|d| {
            item_from_agent_file(ItemKind::Subagent, scope, agent_id, &d, name_from_path)
        })
        .collect()
}

pub fn reconcile_claude(agent: &dyn Agent, scope: Scope, project_root: &Path) -> Vec<Item> {
    reconcile(agent, scope, project_root, AgentId::ClaudeCode, false)
}

pub fn reconcile_opencode(agent: &dyn Agent, scope: Scope, project_root: &Path) -> Vec<Item> {
    reconcile(agent, scope, project_root, AgentId::OpenCode, true)
}

pub fn reconcile_cursor(agent: &dyn Agent, scope: Scope, project_root: &Path) -> Vec<Item> {
    reconcile(agent, scope, project_root, AgentId::Cursor, false)
}

pub fn codex_agents_dir(scope: Scope) -> PathBuf {
    match scope {
        Scope::Global => PathBuf::from("agents"),
        Scope::Project => PathBuf::from(".codex/agents"),
    }
}

fn toml_file_name(item: &Item) -> String {
    format!("{}.toml", item.name())
}

/// Codex sandbox: empty or no write/bash → read-only; otherwise inherit parent.
pub fn format_codex(item: &Item) -> String {
    let mut table = toml::Table::new();
    table.insert("name".into(), toml::Value::String(item.name().to_string()));
    table.insert(
        "description".into(),
        toml::Value::String(item.frontmatter.description.clone()),
    );
    if cursor_readonly(&item.frontmatter.tools) {
        table.insert(
            "sandbox_mode".into(),
            toml::Value::String("read-only".into()),
        );
    }
    if let Some(Value::Mapping(overlay)) = item.frontmatter.native.get(AgentId::Codex.as_str()) {
        for (k, v) in overlay {
            let Some(key) = k.as_str() else {
                continue;
            };
            if matches!(
                key,
                "name" | "description" | "developer_instructions" | "sandbox_mode"
            ) {
                continue;
            }
            if let Some(tv) = yaml_to_toml(v) {
                table.insert(key.to_string(), tv);
            }
        }
    }
    table.insert(
        "developer_instructions".into(),
        toml::Value::String(item.body.trim().to_string()),
    );
    toml::to_string_pretty(&table).unwrap_or_default()
}

fn yaml_to_toml(v: &Value) -> Option<toml::Value> {
    match v {
        Value::Bool(b) => Some(toml::Value::Boolean(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                n.as_f64().map(toml::Value::Float)
            }
        }
        Value::String(s) => Some(toml::Value::String(s.clone())),
        Value::Sequence(seq) => {
            let arr: Option<Vec<_>> = seq.iter().map(yaml_to_toml).collect();
            arr.map(toml::Value::Array)
        }
        _ => None,
    }
}

pub fn render_codex(items: &[Item], scope: Scope) -> Vec<RenderedFile> {
    render_as_directory(
        codex_agents_dir(scope),
        scope,
        items,
        toml_file_name,
        format_codex,
    )
}

pub fn discover_codex(
    agent: &dyn Agent,
    scope: Scope,
    project_root: &Path,
) -> Vec<DiscoveredContent> {
    let Some(root) = agent.root(scope, project_root) else {
        return Vec::new();
    };
    discover_directory(&root.join(codex_agents_dir(scope)), scope, "toml")
}

fn item_from_codex_file(scope: Scope, discovered: &DiscoveredContent) -> Option<Item> {
    let path_name = file_stem_name(&discovered.source_path)?;
    let table: toml::Table = toml::from_str(&discovered.raw).ok()?;
    let name = table
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|n| crate::model::validate_name(n).is_ok())
        .unwrap_or(path_name);
    let description = table
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let body = table
        .get("developer_instructions")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let tools = match table.get("sandbox_mode").and_then(|v| v.as_str()) {
        Some("read-only") => vec!["Read".into(), "Glob".into(), "Grep".into()],
        // Absent / writable modes → explicit full-ish allowlist (empty would
        // re-emit as read-only under the restrictive-empty rule).
        _ => vec![
            "Read".into(),
            "Write".into(),
            "Edit".into(),
            "Bash".into(),
            "Glob".into(),
            "Grep".into(),
            "WebFetch".into(),
        ],
    };
    let mut overlay = serde_yaml_ng::Mapping::new();
    for (k, v) in &table {
        if matches!(
            k.as_str(),
            "name" | "description" | "developer_instructions" | "sandbox_mode"
        ) {
            continue;
        }
        if let Some(yv) = toml_to_yaml(v) {
            overlay.insert(Value::String(k.clone()), yv);
        }
    }
    let mut native = BTreeMap::new();
    if !overlay.is_empty() {
        native.insert(AgentId::Codex.as_str().to_string(), Value::Mapping(overlay));
    }
    let frontmatter = Frontmatter {
        name,
        description,
        applies_to: Vec::new(),
        tags: Vec::new(),
        scope: vec![scope],
        agents: {
            let mut agents = ItemKind::SUBAGENT_AGENTS.to_vec();
            if !agents.contains(&AgentId::Codex) {
                agents.push(AgentId::Codex);
            }
            agents
        },
        tools,
        native,
    };
    Item::new(ItemKind::Subagent, frontmatter, body).ok()
}

fn toml_to_yaml(v: &toml::Value) -> Option<Value> {
    match v {
        toml::Value::Boolean(b) => Some(Value::Bool(*b)),
        toml::Value::Integer(i) => Some(Value::Number((*i).into())),
        toml::Value::Float(f) => {
            let n = serde_yaml_ng::Number::from(*f);
            Some(Value::Number(n))
        }
        toml::Value::String(s) => Some(Value::String(s.clone())),
        toml::Value::Array(arr) => {
            let seq: Option<Vec<_>> = arr.iter().map(toml_to_yaml).collect();
            seq.map(Value::Sequence)
        }
        _ => None,
    }
}

pub fn reconcile_codex(agent: &dyn Agent, scope: Scope, project_root: &Path) -> Vec<Item> {
    discover_unowned(agent, ItemKind::Subagent, scope, project_root)
        .into_iter()
        .filter_map(|d| item_from_codex_file(scope, &d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ContentForm;

    fn item(tools: &[&str], body: &str) -> Item {
        Item::new(
            ItemKind::Subagent,
            Frontmatter {
                name: "reviewer".into(),
                description: "Reviews code".into(),
                applies_to: vec![],
                tags: vec![],
                scope: vec![Scope::Project],
                agents: ItemKind::SUBAGENT_AGENTS.to_vec(),
                tools: tools.iter().map(|s| (*s).to_string()).collect(),
                native: BTreeMap::new(),
            },
            body.into(),
        )
        .unwrap()
    }

    #[test]
    fn cursor_readonly_when_no_write_bash() {
        assert!(cursor_readonly(&["Read".into(), "Grep".into()]));
        assert!(cursor_readonly(&[]), "empty tools must not escalate");
        assert!(!cursor_readonly(&["Bash".into()]));
        assert!(!cursor_readonly(&["Write".into()]));
    }

    #[test]
    fn claude_round_trips_hooks_via_native() {
        let mut native = BTreeMap::new();
        let mut hooks = serde_yaml_ng::Mapping::new();
        hooks.insert(
            Value::String("PreToolUse".into()),
            Value::String("check".into()),
        );
        native.insert(
            AgentId::ClaudeCode.as_str().into(),
            Value::Mapping({
                let mut m = serde_yaml_ng::Mapping::new();
                m.insert(Value::String("hooks".into()), Value::Mapping(hooks));
                m.insert(
                    Value::String("isolation".into()),
                    Value::String("worktree".into()),
                );
                m
            }),
        );
        let mut it = item(&["Read", "Grep"], "Be careful.");
        it.frontmatter.native = native;
        let rendered = format_claude(&it);
        assert!(rendered.contains("hooks:"));
        assert!(rendered.contains("isolation: worktree"));
        assert!(rendered.contains("tools: Read, Grep"));

        let discovered = DiscoveredContent {
            source_path: PathBuf::from("agents/reviewer.md"),
            scope: Scope::Project,
            raw: rendered,
            form: ContentForm::Directory,
        };
        let back = item_from_agent_file(
            ItemKind::Subagent,
            Scope::Project,
            AgentId::ClaudeCode,
            &discovered,
            false,
        )
        .unwrap();
        assert_eq!(back.frontmatter.tools, vec!["Read", "Grep"]);
        assert!(back.frontmatter.native.contains_key("claude-code"));
    }

    #[test]
    fn disallowed_tools_parks_in_native_not_dropped() {
        let discovered = DiscoveredContent {
            source_path: PathBuf::from("agents/reviewer.md"),
            scope: Scope::Project,
            raw: "---\nname: reviewer\ndescription: x\ntools: Read\ndisallowedTools: Bash\n---\n\nBody.\n".into(),
            form: ContentForm::Directory,
        };
        let back = item_from_agent_file(
            ItemKind::Subagent,
            Scope::Project,
            AgentId::ClaudeCode,
            &discovered,
            false,
        )
        .unwrap();
        let native =
            serde_yaml_ng::to_string(back.frontmatter.native.get("claude-code").expect("overlay"))
                .unwrap();
        assert!(
            native.contains("disallowedTools"),
            "disallowedTools must park in native: {native}"
        );
    }

    #[test]
    fn opencode_maps_tools_to_permission() {
        let rendered = format_opencode(&item(&["Read", "Grep"], "Review."));
        assert!(rendered.contains("edit: deny"));
        assert!(rendered.contains("bash: deny"));
        assert!(rendered.contains("mode: subagent"));
    }

    #[test]
    fn opencode_tools_override_sticky_native_permission() {
        let mut it = item(&["Read"], "Review.");
        let mut perm = serde_yaml_ng::Mapping::new();
        // Hostile sticky overlay: allow edit/bash despite tools=[Read]
        perm.insert(Value::String("edit".into()), Value::String("allow".into()));
        perm.insert(Value::String("bash".into()), Value::String("allow".into()));
        let mut overlay = serde_yaml_ng::Mapping::new();
        overlay.insert(Value::String("permission".into()), Value::Mapping(perm));
        overlay.insert(Value::String("color".into()), Value::String("blue".into()));
        it.frontmatter
            .native
            .insert(AgentId::OpenCode.as_str().into(), Value::Mapping(overlay));
        let rendered = format_opencode(&it);
        assert!(
            rendered.contains("edit: deny") && rendered.contains("bash: deny"),
            "tools must beat native permission: {rendered}"
        );
        assert!(
            !rendered.contains("edit: allow") && !rendered.contains("bash: allow"),
            "native must not widen: {rendered}"
        );
        assert!(
            rendered.contains("color: blue"),
            "non-permission native keys still merge: {rendered}"
        );
    }

    #[test]
    fn claude_prefers_frontmatter_name_opencode_uses_path() {
        let raw = "---\nname: other-id\ndescription: x\n---\n\nBody.\n";
        let discovered = DiscoveredContent {
            source_path: PathBuf::from("agents/reviewer.md"),
            scope: Scope::Project,
            raw: raw.into(),
            form: ContentForm::Directory,
        };
        let claude = item_from_agent_file(
            ItemKind::Subagent,
            Scope::Project,
            AgentId::ClaudeCode,
            &discovered,
            false,
        )
        .unwrap();
        assert_eq!(claude.name(), "other-id");
        let oc = item_from_agent_file(
            ItemKind::Subagent,
            Scope::Project,
            AgentId::OpenCode,
            &discovered,
            true,
        )
        .unwrap();
        assert_eq!(oc.name(), "reviewer");
    }

    #[test]
    fn empty_tools_emits_restrictive_opencode_and_codex() {
        let rendered = format_opencode(&item(&[], "Body."));
        assert!(rendered.contains("edit: deny"));
        assert!(rendered.contains("bash: deny"));
        let codex = format_codex(&item(&[], "Body."));
        assert!(codex.contains("sandbox_mode = \"read-only\""), "{codex}");
    }

    #[test]
    fn format_cursor_emits_readonly_for_read_only_tools() {
        let rendered = format_cursor(&item(&["Read", "Grep"], "Review."));
        assert!(
            rendered.contains("readonly: true"),
            "expected readonly: {rendered}"
        );
        let with_edit = format_cursor(&item(&["Read", "Edit"], "Review."));
        assert!(
            !with_edit.contains("readonly: true"),
            "Edit must not be readonly: {with_edit}"
        );
    }
}
