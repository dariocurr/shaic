use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{AgentId, Scope, validate_name};
use crate::security::secrets;

/// One MCP server definition in the canonical store. `command`/`args` and
/// non-secret `env` values sync via git like everything else; any `env`
/// value that's a real credential must be an `EnvValue::Secret` reference,
/// resolved locally at materialize time — never a literal in this struct,
/// which is exactly what gets committed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, EnvValue>,
    #[serde(default = "default_scopes")]
    pub scope: Vec<Scope>,
    /// Which agents this server materializes to — same "absent means every
    /// agent" default as `Frontmatter::agents`, for the same reason: a
    /// write-capable server you trust in one agent's permission model isn't
    /// necessarily one you want fanned out to every agent by default.
    #[serde(default = "default_agents")]
    pub agents: Vec<AgentId>,
}

fn default_scopes() -> Vec<Scope> {
    vec![Scope::Global, Scope::Project]
}

fn default_agents() -> Vec<AgentId> {
    AgentId::ALL.to_vec()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvValue {
    Literal(String),
    Secret { secret: String },
}

impl McpServer {
    pub fn new(
        name: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, EnvValue>,
        scope: Vec<Scope>,
    ) -> Result<Self> {
        validate_name(&name)?;
        Ok(McpServer {
            name,
            command,
            args,
            env,
            scope,
            agents: default_agents(),
        })
    }
}

/// Resolve every `env` entry to its real value: literals pass through,
/// secrets are looked up in the local OS keychain. Fails loudly (naming the
/// server and the missing secret) rather than materializing a server with a
/// missing/empty credential.
pub fn resolve_env(server: &McpServer) -> Result<BTreeMap<String, String>> {
    let mut resolved = BTreeMap::new();
    for (key, value) in &server.env {
        let v = match value {
            EnvValue::Literal(s) => s.clone(),
            EnvValue::Secret { secret } => secrets::get(secret)?
                .filter(|v| !v.is_empty())
                .ok_or_else(|| Error::SecretNotSet {
                    server: server.name.clone(),
                    secret: secret.clone(),
                })?,
        };
        resolved.insert(key.clone(), v);
    }
    Ok(resolved)
}

/// Starter template opened in `$EDITOR` for `shaic mcp add`.
pub fn mcp_template(name: &str) -> String {
    format!(
        "name = {name:?}\ncommand = \"\"\nargs = []\nscope = [\"global\", \"project\"]\n# agents = [\"claude-code\"]  # omit for every agent; restrict to keep a\n#                            # write-capable or agent-specific server out\n#                            # of agents it doesn't belong in.\n\n# Non-secret values go here as plain strings. For anything that's a real\n# credential, reference a name set with `shaic mcp secret set <name>`\n# instead of writing the value here — that's the whole point of \"secret\".\n[env]\n# API_TOKEN = {{ secret = \"API_TOKEN\" }}\n"
    )
}

pub fn render_for_edit(server: &McpServer) -> Result<String> {
    toml::to_string_pretty(server).map_err(|e| Error::FrontmatterParse(e.to_string()))
}

pub fn parse_mcp_toml(raw: &str) -> Result<McpServer> {
    let server: McpServer =
        toml::from_str(raw).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    validate_name(&server.name)?;
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_and_secret_env_values() {
        let raw = r#"
name = "github"
command = "npx"
args = ["-y", "server-github"]

[env]
LOG_LEVEL = "debug"
GITHUB_TOKEN = { secret = "GITHUB_TOKEN" }
"#;
        let server = parse_mcp_toml(raw).unwrap();
        assert_eq!(server.name, "github");
        match server.env.get("LOG_LEVEL") {
            Some(EnvValue::Literal(v)) => assert_eq!(v, "debug"),
            other => panic!("expected literal, got {other:?}"),
        }
        match server.env.get("GITHUB_TOKEN") {
            Some(EnvValue::Secret { secret }) => assert_eq!(secret, "GITHUB_TOKEN"),
            other => panic!("expected secret reference, got {other:?}"),
        }
    }

    #[test]
    fn resolve_env_passes_through_literals_without_touching_the_keychain() {
        let server = McpServer::new(
            "test-server".to_string(),
            "echo".to_string(),
            vec![],
            BTreeMap::from([(
                "LOG_LEVEL".to_string(),
                EnvValue::Literal("debug".to_string()),
            )]),
            vec![Scope::Project],
        )
        .unwrap();
        let resolved = resolve_env(&server).unwrap();
        assert_eq!(resolved.get("LOG_LEVEL").map(String::as_str), Some("debug"));
    }

    #[test]
    fn resolve_env_errors_clearly_when_secret_is_not_set() {
        let server = McpServer::new(
            "test-server".to_string(),
            "echo".to_string(),
            vec![],
            BTreeMap::from([(
                "TOKEN".to_string(),
                EnvValue::Secret {
                    secret: "shaic-test-secret-that-should-never-exist-4f9a1c".to_string(),
                },
            )]),
            vec![Scope::Project],
        )
        .unwrap();
        let err = resolve_env(&server).unwrap_err();
        assert!(matches!(err, Error::SecretNotSet { .. }));
    }
}
