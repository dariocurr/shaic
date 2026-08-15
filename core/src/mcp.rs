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
///
/// A server can describe stdio transport (`command`), HTTP transport (`url` +
/// `bearer_token_env_var`), or both — agents pick the shape they can use at
/// materialize time (JSON agents get stdio; Codex can get either).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, EnvValue>,
    /// Hosted MCP endpoint for HTTP transport (Codex `[mcp_servers.*].url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Env var *name* Codex reads the bearer token from at launch — stored as
    /// a secret reference so the canonical store never holds the token value.
    /// Materialized literally as the var name, not resolved into the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<EnvValue>,
    #[serde(default = "default_scopes")]
    pub scope: Vec<Scope>,
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
        let server = McpServer {
            name,
            command,
            args,
            env,
            url: None,
            bearer_token_env_var: None,
            scope,
            agents: default_agents(),
        };
        server.validate()?;
        Ok(server)
    }

    pub fn has_stdio(&self) -> bool {
        !self.command.is_empty()
    }

    pub fn has_http(&self) -> bool {
        self.url.as_ref().is_some_and(|u| !u.is_empty())
    }

    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        if !self.has_stdio() && !self.has_http() {
            return Err(Error::FrontmatterParse(
                "MCP server needs a non-empty `command` (stdio) and/or `url` (http)".to_string(),
            ));
        }
        Ok(())
    }

    /// What a human should see before applying an MCP write. Command and args
    /// are the remote-code-execution surface, so they must be visible; env
    /// values are not, so they never appear here.
    pub fn transport_summary(&self) -> String {
        let stdio = if self.has_stdio() {
            let mut s = self.command.clone();
            for arg in &self.args {
                s.push(' ');
                s.push_str(arg);
            }
            Some(s)
        } else {
            None
        };
        let http = self
            .url
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(|u| format!("http {}", crate::store::git::redact_userinfo(u)));
        match (stdio, http) {
            (Some(s), Some(h)) => format!("{s} + {h}"),
            (Some(s), None) => s,
            (None, Some(h)) => h,
            (None, None) => String::new(),
        }
    }
}

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

/// Resolve `bearer_token_env_var` to the env var *name* Codex should read at
/// launch. Verifies the referenced secret is set on this machine but never
/// returns the token value — Codex reads it from the process environment.
pub fn resolve_bearer_env_var_name(server: &McpServer) -> Result<Option<String>> {
    let Some(value) = &server.bearer_token_env_var else {
        return Ok(None);
    };
    let name = match value {
        EnvValue::Literal(s) if !s.is_empty() => s.clone(),
        EnvValue::Literal(_) => {
            return Err(Error::FrontmatterParse(format!(
                "{}: bearer_token_env_var must not be empty",
                server.name
            )));
        }
        EnvValue::Secret { secret } => {
            secrets::get(secret)?
                .filter(|v| !v.is_empty())
                .ok_or_else(|| Error::SecretNotSet {
                    server: server.name.clone(),
                    secret: secret.clone(),
                })?;
            secret.clone()
        }
    };
    Ok(Some(name))
}

pub fn mcp_template(name: &str) -> String {
    format!(
        "name = {name:?}\ncommand = \"\"\nargs = []\nscope = [\"global\", \"project\"]\n# agents = [\"claude-code\"]  # omit for every agent; restrict to keep a\n#                            # write-capable or agent-specific server out\n#                            # of agents it doesn't belong in.\n\n# --- stdio transport (Cursor, Claude Code, Windsurf, Copilot) ---\n# command = \"npx\"\n# args = [\"-y\", \"@modelcontextprotocol/server-example\"]\n\n# --- HTTP transport (Codex) ---\n# url = \"https://mcp.example.com/\"\n# bearer_token_env_var = {{ secret = \"MCP_BEARER_TOKEN\" }}\n# agents = [\"codex\"]\n\n# Non-secret env values are plain strings. Real credentials use\n# `shaic mcp secret set <name>` and a {{ secret = \"...\" }} reference.\n[env]\n# API_TOKEN = {{ secret = \"API_TOKEN\" }}\n"
    )
}

pub fn render_for_edit(server: &McpServer) -> Result<String> {
    toml::to_string_pretty(server).map_err(|e| Error::FrontmatterParse(e.to_string()))
}

pub fn parse_mcp_toml(raw: &str) -> Result<McpServer> {
    let server: McpServer =
        toml::from_str(raw).map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    server.validate()?;
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
    fn parses_http_codex_server() {
        let raw = r#"
name = "remote-tools"
url = "https://mcp.example.com/"
bearer_token_env_var = { secret = "MCP_BEARER_TOKEN" }
agents = ["codex"]
"#;
        let server = parse_mcp_toml(raw).unwrap();
        assert!(!server.has_stdio());
        assert!(server.has_http());
    }

    #[test]
    fn rejects_server_with_no_transport() {
        let raw = r#"name = "empty""#;
        assert!(parse_mcp_toml(raw).is_err());
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
        let _guard = crate::security::secrets::ForceMissingSecrets::enable();
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

    #[test]
    fn transport_summary_shows_command_and_redacts_url_never_env() {
        let server = McpServer {
            name: "tools".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server-example".to_string()],
            env: BTreeMap::from([(
                "API_TOKEN".to_string(),
                EnvValue::Literal("super-secret-value-xyz".to_string()),
            )]),
            url: Some("https://user:hunter2@mcp.example.com/".to_string()),
            bearer_token_env_var: None,
            scope: vec![Scope::Project],
            agents: default_agents(),
        };
        let summary = server.transport_summary();
        assert!(summary.contains("npx -y server-example"), "{summary}");
        assert!(summary.contains("mcp.example.com"), "{summary}");
        assert!(!summary.contains("super-secret-value-xyz"), "{summary}");
        assert!(!summary.contains("hunter2"), "{summary}");
        assert!(!summary.contains("user:"), "{summary}");
    }
}
