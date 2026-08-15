pub mod agents;
pub mod doctor;
pub mod import;
pub mod init;
pub mod item;
pub mod mcp;
pub mod project;
pub mod pull;
pub mod push;
pub mod self_cmd;
pub mod status;
pub mod sync;
pub mod tui;

use std::io::Write;
use std::path::PathBuf;

use shaic_core::config::Config;
use shaic_core::model::{AgentId, Scope};
use shaic_core::store::Store;

use crate::error::{Context, Result, bail};

pub fn current_project_root() -> Result<PathBuf> {
    shaic_core::config::infer_project_root().context("could not determine current directory")
}

pub fn open_store() -> Result<Store> {
    Ok(Store::open(Store::default_path()?)?)
}

pub struct Targets {
    pub agents: Vec<AgentId>,
    pub scopes: Vec<Scope>,
    pub project_root: PathBuf,
}

pub fn resolve_targets(
    agents: Vec<AgentId>,
    global: bool,
    project: bool,
    all: bool,
    register_project: bool,
) -> Result<Targets> {
    let mut config = Config::load()?;
    let project_root = current_project_root()?;
    let scopes: Vec<Scope> = if all || (!global && !project) {
        vec![Scope::Global, Scope::Project]
    } else {
        let mut s = Vec::new();
        if global {
            s.push(Scope::Global);
        }
        if project {
            s.push(Scope::Project);
        }
        s
    };
    if register_project && scopes.contains(&Scope::Project) {
        config.ensure_project_registered(&project_root)?;
    }
    let agents = if agents.is_empty() {
        config.enabled_agent_ids()
    } else {
        agents
    };
    Ok(Targets {
        agents,
        scopes,
        project_root,
    })
}

pub fn confirm(prompt: &str) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        bail!("stdin is not a terminal — pass --yes to confirm non-interactively");
    }
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim().to_lowercase().as_str(), "y" | "yes"))
}
