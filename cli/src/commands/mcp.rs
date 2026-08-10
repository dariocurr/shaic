use shaic_core::materialize;
use shaic_core::mcp::{self, EnvValue};
use shaic_core::security::secrets;

use crate::error::Result;
use crate::{McpAction, McpSecretAction};

use super::{current_project_root, open_store};

pub fn run(action: McpAction) -> Result<()> {
    match action {
        McpAction::Add { name } => {
            let store = open_store()?;
            let raw = shaic_core::editor::edit_in_editor(&mcp::mcp_template(&name))?;
            let server = mcp::parse_mcp_toml(&raw)?;
            store.save_mcp_server(&server)?;
            println!("added MCP server {:?}", server.name);
            Ok(())
        }
        McpAction::Edit { name } => {
            let store = open_store()?;
            let existing = store.load_mcp_server(&name)?;
            let raw = shaic_core::editor::edit_in_editor(&mcp::render_for_edit(&existing)?)?;
            let server = mcp::parse_mcp_toml(&raw)?;
            store.save_mcp_server(&server)?;
            println!("updated MCP server {:?}", server.name);
            Ok(())
        }
        McpAction::Rm { name } => {
            let store = open_store()?;
            store.remove_mcp_server(&name)?;
            println!("removed MCP server {name:?}");
            let project_root = current_project_root()?;
            let (applied, notes) = materialize::push_all_now(&store, &project_root);
            for note in &notes {
                println!("[skip] {note}");
            }
            println!("pushed the removal to {applied} agent/scope(s)");
            Ok(())
        }
        McpAction::List => {
            let store = open_store()?;
            let (servers, skipped) = store.list_mcp_servers()?;
            for server in servers {
                let secret_names: Vec<&str> = server
                    .env
                    .iter()
                    .filter_map(|(k, v)| matches!(v, EnvValue::Secret { .. }).then_some(k.as_str()))
                    .collect();
                // Only called out when restricted — a server targeting every
                // agent (the common case) stays silent here.
                let agents = if server.agents.len() < shaic_core::model::AgentId::ALL.len() {
                    format!(
                        "\tagents=[{}]",
                        server
                            .agents
                            .iter()
                            .map(shaic_core::model::AgentId::as_str)
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                } else {
                    String::new()
                };
                println!(
                    "{}\t{}\tsecrets=[{}]{agents}",
                    server.name,
                    server.command,
                    secret_names.join(",")
                );
            }
            for (_, message) in skipped {
                println!("[skip] {message}");
            }
            Ok(())
        }
        McpAction::Secret(action) => run_secret(action),
    }
}

fn run_secret(action: McpSecretAction) -> Result<()> {
    match action {
        McpSecretAction::Set { name } => {
            let value = read_secret_value(&name)?;
            if value.is_empty() {
                println!("empty value entered — secret {name:?} left unset");
                return Ok(());
            }
            secrets::set(&name, &value)?;
            println!("secret {name:?} set");
            Ok(())
        }
        McpSecretAction::Rm { name } => {
            secrets::remove(&name)?;
            println!("secret {name:?} removed");
            Ok(())
        }
        McpSecretAction::List => {
            for name in secrets::list_names()? {
                println!("{name}");
            }
            Ok(())
        }
    }
}

/// Interactively, this hides the input (via the terminal, not stdin) so it
/// never echoes to the screen or lands in shell history/scrollback. When
/// stdin isn't a terminal (piped from a password manager, a CI secret, a
/// script) there's no terminal to hide input on anyway, so read a line from
/// stdin directly instead — `rpassword`'s prompt otherwise unconditionally
/// opens `/dev/tty` and hard-fails wherever one doesn't exist.
fn read_secret_value(name: &str) -> Result<String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        Ok(rpassword::prompt_password(format!(
            "value for secret {name:?} (input hidden, stored in this machine's OS keychain, never synced): "
        ))?)
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    }
}
