use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentForm {
    SingleFile,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Skill,
    Rule,
    Command,
}

impl ItemKind {
    pub const ALL: [ItemKind; 3] = [ItemKind::Skill, ItemKind::Rule, ItemKind::Command];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    ClaudeCode,
    Cursor,
    Windsurf,
    Copilot,
    Codex,
    OpenCode,
    Gemini,
    Antigravity,
    Cline,
}

impl AgentId {
    pub const ALL: [AgentId; 9] = [
        AgentId::ClaudeCode,
        AgentId::Cursor,
        AgentId::Windsurf,
        AgentId::Copilot,
        AgentId::Codex,
        AgentId::OpenCode,
        AgentId::Gemini,
        AgentId::Antigravity,
        AgentId::Cline,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            AgentId::ClaudeCode => "claude-code",
            AgentId::Cursor => "cursor",
            AgentId::Windsurf => "windsurf",
            AgentId::Copilot => "copilot",
            AgentId::Codex => "codex",
            AgentId::OpenCode => "opencode",
            AgentId::Gemini => "gemini",
            AgentId::Antigravity => "antigravity",
            AgentId::Cline => "cline",
        }
    }
}

/// Frontmatter carried by every canonical item. Intentionally has no path-shaped
/// field: the only way a name becomes a path segment is `name` itself, which is
/// sanitized in `Item::new`/`validate_name` before it can reach any adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub applies_to: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_scopes")]
    pub scope: Vec<Scope>,
    /// Which agents this item materializes to. Defaults to every known
    /// agent (the field simply being absent means "fan out everywhere",
    /// matching how `scope` defaults to both) — set explicitly to a subset
    /// to keep an agent-specific item (one that leans on Claude Code hooks/
    /// subagents, say) out of agents it doesn't apply to.
    #[serde(default = "default_agents")]
    pub agents: Vec<AgentId>,
}

impl Frontmatter {
    /// Top-level keys this build knows. `parse_lenient` drops anything else
    /// so an older client isn't bricked by a field a newer shaic added.
    /// Keep in lockstep with the struct fields above — a missing name here
    /// would silently discard a real field on every load.
    pub const FIELDS: &[&str] = &[
        "name",
        "description",
        "applies_to",
        "tags",
        "scope",
        "agents",
    ];
}

fn default_scopes() -> Vec<Scope> {
    vec![Scope::Global, Scope::Project]
}

fn default_agents() -> Vec<AgentId> {
    AgentId::ALL.to_vec()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub frontmatter: Frontmatter,
    pub body: String,
}

impl Item {
    pub fn new(kind: ItemKind, frontmatter: Frontmatter, body: String) -> Result<Self> {
        validate_name(&frontmatter.name)?;
        Ok(Item {
            kind,
            frontmatter,
            body,
        })
    }

    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }
}

/// Reject anything that isn't a single, plain path component: no `/`, `\`, NUL,
/// `..`, and nothing that resolves to more (or fewer) than one `Normal` component.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\0') || name.contains('/') || name.contains('\\') {
        return Err(Error::InvalidName(name.to_string()));
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(c)), None) if c.to_str() == Some(name) => Ok(()),
        _ => Err(Error::InvalidName(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_names() {
        for bad in ["../../etc/passwd", "..", "a/b", "a\\b", "", "a/../b", "."] {
            assert!(
                validate_name(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn accepts_plain_names() {
        for good in ["code-review-checklist", "no_any_in_ts", "a"] {
            assert!(
                validate_name(good).is_ok(),
                "expected {good:?} to be accepted"
            );
        }
    }
}
