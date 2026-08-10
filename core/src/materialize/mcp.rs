use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::adapters::Agent;
use crate::error::{Error, Result};
use crate::mcp::{EnvValue, McpServer, resolve_env};
use crate::model::{Scope, validate_name};
use crate::security::{path_guard, secrets};
use crate::store::Store;

use super::writer::{self, Manifest, WriteAction};

/// The directory `mcp_target()` paths are trusted to live under — the same
/// boundary `Agent::root()` uses for items (`project_root` for
/// `Scope::Project`, the home directory for `Scope::Global`). Every MCP write
/// is validated against this via `path_guard::ensure_within` before it
/// happens, exactly like every other write in the crate.
fn scope_base(scope: Scope, project_root: &Path) -> PathBuf {
    match scope {
        Scope::Project => project_root.to_path_buf(),
        Scope::Global => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
    }
}

#[derive(Debug, Clone)]
pub struct PlannedMcpWrite {
    pub name: String,
    pub action: WriteAction,
}

#[derive(Debug, Default)]
pub struct McpPlan {
    pub writes: Vec<PlannedMcpWrite>,
    pub removals: Vec<String>,
    pub skipped: Vec<String>,
}

impl McpPlan {
    pub fn is_empty(&self) -> bool {
        self.writes.iter().all(|w| w.action == WriteAction::NoOp) && self.removals.is_empty()
    }

    pub fn changed_writes(&self) -> impl Iterator<Item = &PlannedMcpWrite> {
        self.writes.iter().filter(|w| w.action != WriteAction::NoOp)
    }
}

/// Compute what materializing MCP servers would change for one agent+scope,
/// without writing or resolving-into-any-file anything. Secrets ARE resolved
/// in memory here (to know whether an entry actually changed), but the plan
/// only ever records a server *name* and an action — never the resolved
/// value — so a diff preview built from this can't leak a credential.
pub fn plan_mcp(
    agent: &dyn Agent,
    store: &Store,
    scope: Scope,
    project_root: &Path,
) -> Result<McpPlan> {
    let mut plan = McpPlan::default();
    let Some(target) = agent.mcp_target(scope, project_root) else {
        plan.skipped.push(format!(
            "{} does not support materializing MCP servers in {scope:?} scope",
            agent.display_name()
        ));
        return Ok(plan);
    };

    let (all_servers, skipped_servers) = store.list_mcp_servers()?;
    let servers: Vec<McpServer> = all_servers
        .into_iter()
        .filter(|s| s.scope.contains(&scope) && s.agents.contains(&agent.id()))
        .collect();
    let existing = read_managed_object(&target.path, target.servers_key)?;

    let mut live_names = Vec::new();
    for (name, message) in &skipped_servers {
        // A store file that failed to load is neither confirmed present nor
        // confirmed gone — if it has a valid name, treat it as live so a
        // parse error can never look like the server left the store and get
        // it removed from every agent's config.
        if !name.is_empty() {
            live_names.push(name.clone());
        }
        plan.skipped.push(message.clone());
    }
    for server in &servers {
        // Tracked as live regardless of whether it resolves below, so a
        // server with a not-yet-set secret is never mistaken for one that
        // left the store and queued for removal.
        live_names.push(server.name.clone());
        let resolved_env = match resolve_env(server) {
            Ok(env) => env,
            // A missing secret on *this* server must not abort planning for
            // every other agent/scope in the same `shaic status`/`sync`
            // call — report it and move on, the same way a malformed store
            // file is skipped rather than propagated.
            Err(Error::SecretNotSet {
                server: server_name,
                secret,
            }) => {
                plan.skipped.push(format!(
                    "{server_name:?} needs secret {secret:?}, which isn't set on this machine (run `shaic mcp secret set {secret}`) — skipped"
                ));
                continue;
            }
            Err(e) => return Err(e),
        };
        let candidate = to_json_entry(server, &resolved_env);
        let action = match existing.get(&server.name) {
            Some(current) if *current == candidate => WriteAction::NoOp,
            Some(_) => WriteAction::Update,
            None => WriteAction::Create,
        };
        plan.writes.push(PlannedMcpWrite {
            name: server.name.clone(),
            action,
        });
    }

    let manifest = Manifest::load(&Manifest::mcp_path_for(agent.id(), scope));
    for tracked in manifest.tracked_paths() {
        if live_names.iter().any(|n| n == tracked) {
            continue;
        }
        if let Some(current) = existing.get(tracked)
            && manifest.safe_to_delete(tracked, &manifest_key(current))
        {
            plan.removals.push(tracked.to_string());
        }
    }

    Ok(plan)
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    /// Server names pulled from the agent's on-disk config into the store.
    pub pulled: Vec<String>,
    /// `(name, reason)` — entries that looked like a real server but were
    /// rejected going into the store (most commonly the secret-scan
    /// tripwire catching an obviously-shaped credential typed as a literal).
    pub rejected: Vec<(String, String)>,
}

/// Pull an agent's on-disk MCP server entries back into the canonical store,
/// so a server added or edited directly in one agent's config (bypassing
/// `shaic mcp add`/`edit`) propagates to every other agent on the same
/// `sync` run instead of staying invisible to the store — or getting
/// silently overwritten by the store's stale copy the next time this agent
/// itself is materialized.
///
/// Callers must only invoke this from a real, confirmed apply — never from
/// a `--dry-run`/Diff Preview path — since, unlike `plan_mcp`, it writes
/// into the store immediately rather than returning a plan to review first.
///
/// There is no way to tell a hand-typed literal credential apart from a
/// resolved `{ secret = "NAME" }` reference once it's sitting in an agent's
/// config as plain text — except by checking whether it still matches what
/// shaic itself last resolved for that name, in which case the reference is
/// kept. Anything else becomes a literal `env` value, which
/// `Store::save_mcp_server`'s secret-scan tripwire still screens for
/// obviously-shaped credentials before it's allowed into the store.
pub fn reconcile_mcp(
    agent: &dyn Agent,
    store: &Store,
    scope: Scope,
    project_root: &Path,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    let Some(target) = agent.mcp_target(scope, project_root) else {
        return Ok(report);
    };
    // A target that doesn't exist yet, or whose managed key is some other
    // shape, has nothing to reconcile — `plan_mcp`/`apply_mcp` will report
    // a real shape problem when they run right after this.
    let Ok(on_disk) = read_managed_object(&target.path, target.servers_key) else {
        return Ok(report);
    };
    let (known_servers, _) = store.list_mcp_servers()?;

    for (name, value) in &on_disk {
        if validate_name(name).is_err() {
            continue;
        }
        let Some(obj) = value.as_object() else {
            continue;
        };
        let Some(command) = obj.get("command").and_then(|v| v.as_str()) else {
            continue; // not a stdio server shaic's model can represent (e.g. remote/http)
        };
        let args: Vec<String> = obj
            .get("args")
            .and_then(|v| v.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let existing = known_servers.iter().find(|s| s.name == *name);
        let mut env = BTreeMap::new();
        if let Some(env_obj) = obj.get("env").and_then(|v| v.as_object()) {
            for (key, value) in env_obj {
                if let Some(on_disk) = value.as_str() {
                    let previous = existing.and_then(|s| s.env.get(key));
                    env.insert(key.clone(), reconciled_env_value(previous, on_disk));
                }
            }
        }

        // Keep every scope the store already had this server materializing
        // into, plus this one — so pulling a change made via one agent
        // doesn't quietly drop the server from scopes it already covered.
        let mut scopes = existing.map(|s| s.scope.clone()).unwrap_or_default();
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }

        let candidate = match McpServer::new(name.clone(), command.to_string(), args, env, scopes) {
            Ok(c) => c,
            Err(e) => {
                report.rejected.push((name.clone(), e.to_string()));
                continue;
            }
        };
        if existing == Some(&candidate) {
            continue; // already matches the store — nothing to pull
        }
        match store.save_mcp_server(&candidate) {
            Ok(()) => report.pulled.push(name.clone()),
            Err(e) => report.rejected.push((name.clone(), e.to_string())),
        }
    }
    Ok(report)
}

/// Decide what one `env` entry read back out of an agent's config becomes in
/// the store. `previous` is whatever the store already had for the same key:
/// a `{ secret = "NAME" }` reference is only preserved as one when the on-disk
/// text still matches exactly what that secret resolves to on this machine
/// (see `reconcile_mcp` for why that's the only available signal).
fn reconciled_env_value(previous: Option<&EnvValue>, on_disk: &str) -> EnvValue {
    let kept_secret = previous
        .and_then(|prev| match prev {
            EnvValue::Secret { secret } => Some(secret.clone()),
            EnvValue::Literal(_) => None,
        })
        .filter(|secret| secrets::get(secret).ok().flatten().as_deref() == Some(on_disk));
    match kept_secret {
        Some(secret) => EnvValue::Secret { secret },
        None => EnvValue::Literal(on_disk.to_string()),
    }
}

/// Execute a previously computed plan: resolve each changed server's secrets
/// fresh (not carried over from `plan_mcp`, to keep their time in memory as
/// short as possible), merge into the target file's managed key, and update
/// the per-server provenance manifest.
pub fn apply_mcp(
    agent: &dyn Agent,
    store: &Store,
    plan: &McpPlan,
    scope: Scope,
    project_root: &Path,
) -> Result<usize> {
    let Some(target) = agent.mcp_target(scope, project_root) else {
        return Ok(0);
    };

    // Checked before the no-op short-circuit below, and based on every live
    // server for this scope rather than just this call's changed writes — so
    // a project that already has a secret-bearing server synced (from before
    // this check existed, or from a previous run that predates a
    // `.gitignore` entry being added by hand) still gets covered the next
    // time `apply_mcp` runs, not only on the write that first introduces it.
    if scope == Scope::Project {
        let relative_target = target.path.strip_prefix(project_root).map_err(|_| {
            Error::Git(format!(
                "{} is not inside project root {} — refusing to write a possible secret without a gitignore check",
                target.path.display(),
                project_root.display()
            ))
        })?;
        ensure_gitignored_if_holds_secrets(store, scope, relative_target, project_root)?;
    }

    if plan.is_empty() {
        // Nothing changed — skip the read-modify-write round-trip entirely so
        // an already-in-sync file is never rewritten/reformatted underneath
        // whatever hand-editing or comment style the agent (or its user)
        // already put there.
        return Ok(0);
    }
    let mut object = read_managed_object(&target.path, target.servers_key)?;
    let manifest_path = Manifest::mcp_path_for(agent.id(), scope);
    let mut manifest = Manifest::load(&manifest_path);
    let mut applied = 0usize;

    for write in &plan.writes {
        if write.action == WriteAction::NoOp {
            continue;
        }
        let server = store.load_mcp_server(&write.name)?;
        let resolved_env = resolve_env(&server)?;
        let entry = to_json_entry(&server, &resolved_env);
        manifest.record(&write.name, &manifest_key(&entry));
        object.insert(write.name.clone(), entry);
        applied += 1;
    }

    for name in &plan.removals {
        object.remove(name);
        manifest.forget(name);
        applied += 1;
    }

    let base = scope_base(scope, project_root);
    write_managed_object(&target.path, target.servers_key, &object, &base)?;
    manifest.save(&manifest_path)?;
    Ok(applied)
}

/// Project-scope MCP config files (`.mcp.json`, `.cursor/mcp.json`,
/// `.vscode/mcp.json`) live inside the user's repo and are otherwise meant to
/// be committed — that's the whole point of syncing MCP server *definitions*
/// via git. A resolved secret value landing in one is exactly the leak the
/// once-per-machine keychain design exists to prevent, so before any such
/// file can hold one, make sure the project's `.gitignore` covers it.
/// `Scope::Global` never reaches this — those targets live under the home
/// directory, not inside a repo. This is a best-effort exact-line check, not
/// full `.gitignore` glob semantics, and it doesn't stop `git add -f` — it
/// stops the accidental commit, which is the realistic threat.
fn ensure_gitignored_if_holds_secrets(
    store: &Store,
    scope: Scope,
    relative_target: &Path,
    project_root: &Path,
) -> Result<()> {
    let holds_secret = store.list_mcp_servers()?.0.into_iter().any(|s| {
        s.scope.contains(&scope) && s.env.values().any(|v| matches!(v, EnvValue::Secret { .. }))
    });
    if !holds_secret {
        return Ok(());
    }

    let pattern = relative_target.to_string_lossy().replace('\\', "/");
    let gitignore_path = project_root.join(".gitignore");
    // Same trust boundary and write path as every other file this crate
    // touches — a `.gitignore` that's itself a symlink (or whose parent is)
    // must not let this escape `project_root`, and a read error other than
    // "doesn't exist yet" must not be silently treated as an empty file and
    // clobber whatever's actually there.
    let target = path_guard::ensure_within(project_root, &gitignore_path)?;
    let existing = match std::fs::read_to_string(&target) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(Error::Io {
                path: target,
                source,
            });
        }
    };
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&format!(
        "\n# added by shaic: this file can hold a resolved MCP secret value\n{pattern}\n"
    ));
    writer::write_atomic(project_root, &target, &updated, 0o644)
}

/// The string the provenance manifest hashes for a server entry — everything
/// `to_json_entry` produces *except* `env`. Resolved secret values live in
/// `env`, and this hash is persisted in a per-machine state file outside the
/// git-tracked store; hashing the plaintext credential would turn that file
/// into an offline guess-verification oracle for it. The trade-off: a
/// hand-edit that changes only an `env` value (with `command`/`args`
/// untouched) won't be detected as tampering — command/args are what
/// actually matters for the "still exactly what shaic wrote" delete-safety
/// check.
fn manifest_key(entry: &serde_json::Value) -> String {
    let mut redacted = entry.clone();
    if let Some(obj) = redacted.as_object_mut() {
        obj.remove("env");
    }
    serde_json::to_string(&redacted).unwrap_or_default()
}

fn to_json_entry(server: &McpServer, resolved_env: &BTreeMap<String, String>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("command".to_string(), server.command.clone().into());
    // `args` and `env` are omitted entirely rather than written empty, so
    // shaic's output matches what a human would hand-write for a bare server.
    if !server.args.is_empty() {
        obj.insert("args".to_string(), server.args.clone().into());
    }
    if !resolved_env.is_empty() {
        let env: serde_json::Map<String, serde_json::Value> = resolved_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone().into()))
            .collect();
        obj.insert("env".to_string(), env.into());
    }
    obj.into()
}

fn read_top_level(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
                Error::FrontmatterParse(format!("{}: invalid JSON: {e}", path.display()))
            })?;
            match value {
                serde_json::Value::Object(map) => Ok(map),
                _ => Err(Error::FrontmatterParse(format!(
                    "{}: expected a JSON object at the top level",
                    path.display()
                ))),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_managed_object(
    path: &Path,
    key: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let top = read_top_level(path)?;
    match top.get(key) {
        Some(serde_json::Value::Object(inner)) => Ok(inner.clone()),
        None => Ok(serde_json::Map::new()),
        // Refuse to treat some other shape (array, string, ...) as "empty" —
        // that would make `write_managed_object` silently clobber whatever
        // was actually there with a fresh object.
        Some(other) => Err(Error::FrontmatterParse(format!(
            "{}: {key:?} is {}, not a JSON object — refusing to merge MCP servers into it",
            path.display(),
            json_type_name(other)
        ))),
    }
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Rewrite just `key` in whatever JSON object already lives at `path`,
/// leaving every other top-level key untouched. `base` is the same trust
/// boundary `Agent::root()` uses for items — validated (and, unlike item
/// writes, actually created if this is the first MCP sync for this agent)
/// via `path_guard::ensure_within` before anything is written. Written with
/// mode `0o600`, not the `0o644` item writes use, since this file may hold a
/// resolved credential value.
fn write_managed_object(
    path: &Path,
    key: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    base: &Path,
) -> Result<()> {
    let target = path_guard::ensure_within(base, path)?;
    let mut top = read_top_level(&target)?;
    top.insert(key.to_string(), serde_json::Value::Object(object.clone()));
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(top))
        .map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    text.push('\n');
    writer::write_atomic(base, &target, &text, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_json_entry_omits_empty_args_and_env() {
        let server = McpServer::new(
            "bare".to_string(),
            "npx".to_string(),
            vec![],
            BTreeMap::new(),
            vec![Scope::Project],
        )
        .unwrap();
        let entry = to_json_entry(&server, &BTreeMap::new());
        let obj = entry.as_object().unwrap();
        assert_eq!(obj.get("command").unwrap(), "npx");
        assert!(!obj.contains_key("args"));
        assert!(!obj.contains_key("env"));
    }

    #[test]
    fn write_managed_object_preserves_unrelated_top_level_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"unrelatedSetting": true, "mcpServers": {}}"#).unwrap();

        let mut object = serde_json::Map::new();
        object.insert("github".to_string(), serde_json::json!({"command": "npx"}));
        write_managed_object(&path, "mcpServers", &object, dir.path()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["unrelatedSetting"], serde_json::json!(true));
        assert_eq!(value["mcpServers"]["github"]["command"], "npx");
    }

    #[test]
    fn read_managed_object_defaults_to_empty_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let object = read_managed_object(&path, "mcpServers").unwrap();
        assert!(object.is_empty());
    }
}
