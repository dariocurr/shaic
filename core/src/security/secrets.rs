use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::Store;

/// Every MCP secret lives under this one keychain service name, keyed by the
/// secret's name — set on this machine only, via `shaic mcp secret set`.
/// Never written to the git-tracked store, never synced: this is the entire
/// reason `EnvValue::Secret` exists instead of a literal env value.
const SERVICE: &str = "shaic-mcp";

pub fn set(name: &str, value: &str) -> Result<()> {
    crate::model::validate_name(name)?;
    entry(name)?.set_password(value).map_err(keyring_err)?;
    index::add(name)
}

/// `Ok(None)` means the secret genuinely isn't set — not a load failure.
pub fn get(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(keyring_err(e)),
    }
}

pub fn remove(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(keyring_err(e)),
    }
    index::remove(name)
}

/// Names only — the whole point of the index is that listing what's set
/// never requires reading a value back out of the keychain.
pub fn list_names() -> Result<Vec<String>> {
    index::load()
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, name).map_err(keyring_err)
}

fn keyring_err(e: keyring::Error) -> Error {
    Error::Secret(e.to_string())
}

/// A local, non-secret record of which secret *names* have been set, so
/// `list`/`rm` work without an OS-level "enumerate this service's entries"
/// API (keychains generally don't offer one). Lives under `Store::state_dir()`
/// — per-machine, never git-tracked, same as the materialize provenance
/// manifest.
mod index {
    use super::*;

    fn path() -> PathBuf {
        Store::state_dir().join("mcp-secrets-index.toml")
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct Index {
        #[serde(default)]
        names: BTreeSet<String>,
    }

    fn load_full() -> Index {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Result<Vec<String>> {
        Ok(load_full().names.into_iter().collect())
    }

    pub fn add(name: &str) -> Result<()> {
        let mut idx = load_full();
        idx.names.insert(name.to_string());
        save(&idx)
    }

    pub fn remove(name: &str) -> Result<()> {
        let mut idx = load_full();
        idx.names.remove(name);
        save(&idx)
    }

    fn save(idx: &Index) -> Result<()> {
        let p = path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let toml_str = toml::to_string_pretty(idx).map_err(|e| Error::Config(e.to_string()))?;
        std::fs::write(&p, toml_str).map_err(|source| Error::Io { path: p, source })
    }
}
