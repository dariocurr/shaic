use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::AgentId;
use crate::platform;
use crate::store::git;

/// Everything shaic needs to run, stored per-machine — never synced via git.
/// Safe by construction: there is no field here that could ever hold a
/// credential (the remote URL is validated to reject embedded userinfo).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub remote: Option<String>,
    #[serde(default)]
    pub enabled_agents: Vec<String>,
    #[serde(default)]
    pub projects: Vec<PathBuf>,
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dir = crate::platform::config_dir().ok_or_else(|| {
            Error::Config(
                "no OS config directory — set HOME, XDG_CONFIG_HOME, or APPDATA".to_string(),
            )
        })?;
        Ok(dir.join("shaic").join("config.toml"))
    }

    pub fn load() -> Result<Config> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        toml::from_str(&raw).map_err(|e| Error::Config(e.to_string()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let toml = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        platform::write_private_config(&path, &toml)
    }

    pub fn set_remote(&mut self, url: &str) -> Result<()> {
        git::validate_remote_url(url)?;
        self.remote = Some(url.to_string());
        Ok(())
    }

    pub fn add_project(&mut self, path: PathBuf) {
        let canon = path.canonicalize().unwrap_or(path);
        if !self.projects.contains(&canon) {
            self.projects.push(canon);
        }
    }

    pub fn remove_project(&mut self, path: &Path) {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.projects.retain(|p| p != &canon);
    }

    pub fn is_project_registered(&self, path: &Path) -> bool {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.projects.contains(&canon)
    }

    /// Registers `path` as a project if it isn't already, persisting the
    /// change immediately. Project scope is an addition on top of the
    /// global sync that matters everywhere — it should never block on an
    /// explicit `shaic project add` step first.
    pub fn ensure_project_registered(&mut self, path: &Path) -> Result<()> {
        if !self.is_project_registered(path) {
            self.add_project(path.to_path_buf());
            self.save()?;
        }
        Ok(())
    }

    /// Empty `enabled_agents` means "all of them" — a fresh config doesn't
    /// require the user to opt every agent in by hand.
    pub fn enabled_agent_ids(&self) -> Vec<AgentId> {
        if self.enabled_agents.is_empty() {
            return AgentId::ALL.to_vec();
        }
        AgentId::ALL
            .into_iter()
            .filter(|a| self.enabled_agents.iter().any(|s| s == a.as_str()))
            .collect()
    }
}

/// Directory project-scoped writes should land in: nearest `.git` ancestor of
/// cwd, or cwd itself if none (a tree that hasn't been `git init`ed yet).
pub fn infer_project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().map_err(|source| Error::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let mut dir = cwd.clone();
    loop {
        if dir.join(".git").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Ok(cwd);
        }
    }
}
