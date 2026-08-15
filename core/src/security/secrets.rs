use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

#[cfg(test)]
use std::cell::RefCell;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::platform;
use crate::store::Store;

/// Every MCP secret lives under this one keychain service name, keyed by the
/// secret's name — set on this machine only, via `shaic mcp secret set`.
/// Never written to the git-tracked store, never synced: this is the entire
/// reason `EnvValue::Secret` exists instead of a literal env value.
const SERVICE: &str = "shaic-mcp";

pub fn set(name: &str, value: &str) -> Result<()> {
    crate::model::validate_name(name)?;
    entry(name)?.set_password(value).map_err(keyring_err)?;
    // Index last: if this fails, roll back the keychain write so list/get
    // stay consistent (no orphan credential without a listable name).
    if let Err(e) = index::add(name) {
        let _ = entry(name).and_then(|e| e.delete_credential().map_err(keyring_err));
        return Err(e);
    }
    Ok(())
}

/// `Ok(None)` when the secret isn't set, or the OS secret store isn't
/// reachable on this machine (headless CI, no keyring daemon).
pub fn get(name: &str) -> Result<Option<String>> {
    #[cfg(test)]
    if force_missing_secrets() {
        return Ok(None);
    }

    let entry = match open_entry(name) {
        Ok(e) => e,
        Err(e) if treat_as_secret_not_set(&e) => return Ok(None),
        Err(e) => return Err(keyring_err(e)),
    };
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(e) if treat_as_secret_not_set(&e) => Ok(None),
        Err(e) => Err(keyring_err(e)),
    }
}

/// Reads treat "can't reach store" like "no entry" so MCP resolution can
/// surface `SecretNotSet` instead of an opaque platform error.
fn treat_as_secret_not_set(err: &keyring::Error) -> bool {
    matches!(
        err,
        keyring::Error::NoEntry
            | keyring::Error::PlatformFailure(_)
            | keyring::Error::NoStorageAccess(_)
            | keyring::Error::NoDefaultStore
    )
}

pub fn remove(name: &str) -> Result<()> {
    // Snapshot the value first so an index-update failure after keychain
    // delete can restore the credential instead of leaving a ghost name.
    let previous = get(name)?;
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => return Err(keyring_err(e)),
    }
    if let Err(e) = index::remove(name) {
        if let Some(value) = previous {
            let _ = entry(name).and_then(|ent| ent.set_password(&value).map_err(keyring_err));
        }
        return Err(e);
    }
    Ok(())
}

/// Names only — the whole point of the index is that listing what's set
/// never requires reading a value back out of the keychain.
pub fn list_names() -> Result<Vec<String>> {
    index::load()
}

fn entry(name: &str) -> Result<keyring::Entry> {
    open_entry(name).map_err(keyring_err)
}

fn open_entry(name: &str) -> std::result::Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(SERVICE, name)
}

fn keyring_err(e: keyring::Error) -> Error {
    Error::Secret(e.to_string())
}

#[cfg(test)]
thread_local! {
    static FORCE_MISSING_SECRETS: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(test)]
fn force_missing_secrets() -> bool {
    FORCE_MISSING_SECRETS.with(|flag| *flag.borrow())
}

/// Unit tests that must not depend on the host keychain enable this guard.
#[cfg(test)]
pub struct ForceMissingSecrets;

#[cfg(test)]
impl ForceMissingSecrets {
    pub fn enable() -> Self {
        FORCE_MISSING_SECRETS.with(|flag| *flag.borrow_mut() = true);
        Self
    }
}

#[cfg(test)]
impl Drop for ForceMissingSecrets {
    fn drop(&mut self) {
        FORCE_MISSING_SECRETS.with(|flag| *flag.borrow_mut() = false);
    }
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
        fs::read_to_string(path())
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
        let toml_str = toml::to_string_pretty(idx).map_err(|e| Error::Config(e.to_string()))?;
        platform::write_private_config(&p, &toml_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treat_as_secret_not_set_covers_missing_and_unavailable_store() {
        assert!(treat_as_secret_not_set(&keyring::Error::NoEntry));
        assert!(treat_as_secret_not_set(&keyring::Error::NoDefaultStore));
        assert!(treat_as_secret_not_set(&keyring::Error::PlatformFailure(
            "no keychain".into()
        )));
        assert!(treat_as_secret_not_set(&keyring::Error::NoStorageAccess(
            "locked".into()
        )));
        assert!(!treat_as_secret_not_set(&keyring::Error::BadEncoding(
            vec![0xff]
        )));
    }
}
