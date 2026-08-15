use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Table, Value as TomlValue};

use crate::adapters::{Agent, McpConfigFormat, McpTarget};
use crate::error::{Error, Result};
use crate::mcp::{EnvValue, McpServer, resolve_bearer_env_var_name, resolve_env};
use crate::model::{AgentId, Scope, validate_name};
use crate::security::{path_guard, secrets};
use crate::store::Store;

use super::writer::{self, Manifest, TrackedContent, WriteAction};

/// The directory `mcp_target()` paths are trusted to live under — the same
/// boundary `Agent::root()` uses for items (`project_root` for
/// `Scope::Project`, the home directory for `Scope::Global`). Every MCP write
/// is validated against this via `path_guard::ensure_within` before it
/// happens, exactly like every other write in the crate.
fn scope_base(scope: Scope, project_root: &Path) -> Result<PathBuf> {
    match scope {
        Scope::Project => Ok(project_root.to_path_buf()),
        // Never fall back to `.`: writing MCP config into the current working
        // directory is how a missing `$HOME` used to plant `mcp.json` in
        // whatever folder the user happened to be standing in.
        Scope::Global => crate::platform::home_dir().ok_or_else(|| {
            Error::Config(
                "no home directory on this machine — cannot write global MCP config".to_string(),
            )
        }),
    }
}

#[derive(Debug, Clone)]
pub struct PlannedMcpWrite {
    pub name: String,
    pub action: WriteAction,
    /// Command/args and/or URL. Never env values — those may be resolved
    /// secrets, and a plan is what the CLI/TUI print.
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct McpPlan {
    pub writes: Vec<PlannedMcpWrite>,
    pub removals: Vec<String>,
    pub skipped: Vec<String>,
    /// Non-fatal notes, same contract as `MaterializePlan::warnings`:
    /// returned as data so the CLI can print them and the TUI can fold them
    /// into its own output instead of a library `eprintln!` tearing a hole in
    /// the alternate screen.
    pub warnings: Vec<String>,
}

impl McpPlan {
    pub fn is_empty(&self) -> bool {
        self.writes.iter().all(|w| w.action == WriteAction::NoOp) && self.removals.is_empty()
    }

    pub fn changed_writes(&self) -> impl Iterator<Item = &PlannedMcpWrite> {
        self.writes.iter().filter(|w| w.action != WriteAction::NoOp)
    }
}

/// What `apply_mcp` actually did, plus anything it had to give up on.
#[derive(Debug, Default)]
pub struct McpApplyReport {
    /// Entries written plus entries removed.
    pub applied: usize,
    pub warnings: Vec<String>,
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
    let existing = read_managed_servers(&target)?;

    let mut live_names = Vec::new();
    for (name, message) in &skipped_servers {
        if !name.is_empty() {
            live_names.push(name.clone());
        }
        plan.skipped.push(message.clone());
    }
    for server in &servers {
        live_names.push(server.name.clone());
        let candidate = match materialize_entry(server, &target.format) {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                plan.skipped.push(format!(
                    "{:?} has no transport compatible with {} — skipped",
                    server.name,
                    agent.display_name()
                ));
                continue;
            }
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
        let action = match existing.get(&server.name) {
            Some(current) if *current == candidate => WriteAction::NoOp,
            Some(_) => WriteAction::Update,
            None => WriteAction::Create,
        };
        plan.writes.push(PlannedMcpWrite {
            name: server.name.clone(),
            action,
            summary: server.transport_summary(),
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
    pub pulled: Vec<String>,
    pub rejected: Vec<(String, String)>,
    /// Notes that aren't per-item rejections: an agent whose root couldn't be
    /// resolved at all, or a frontmatter field this build doesn't know.
    pub warnings: Vec<String>,
}

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
    let Ok(on_disk) = read_managed_servers(&target) else {
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
        let existing = known_servers.iter().find(|s| s.name == *name);
        let candidate = match server_from_on_disk_entry(name, obj, existing, scope, agent.id()) {
            Ok(c) => c,
            Err(e) => {
                report.rejected.push((name.clone(), e.to_string()));
                continue;
            }
        };
        if existing == Some(&candidate) {
            continue;
        }
        match store.save_mcp_server(&candidate) {
            Ok(()) => report.pulled.push(name.clone()),
            Err(e) => report.rejected.push((name.clone(), e.to_string())),
        }
    }
    Ok(report)
}

pub fn apply_mcp(
    agent: &dyn Agent,
    store: &Store,
    plan: &McpPlan,
    scope: Scope,
    project_root: &Path,
) -> Result<McpApplyReport> {
    let Some(target) = agent.mcp_target(scope, project_root) else {
        return Ok(McpApplyReport::default());
    };
    let mut report = McpApplyReport::default();

    if scope == Scope::Project {
        let relative_target =
            target
                .path
                .strip_prefix(project_root)
                .map_err(|_| Error::OutsideProjectRoot {
                    path: target.path.clone(),
                    project_root: project_root.to_path_buf(),
                })?;
        ensure_gitignored_if_holds_secrets(
            store,
            agent,
            scope,
            relative_target,
            project_root,
            &mut report.warnings,
        )?;
    }

    if plan.is_empty() {
        return Ok(report);
    }
    let mut object = read_managed_servers(&target)?;
    let manifest_path = Manifest::mcp_path_for(agent.id(), scope);
    let mut manifest = Manifest::load(&manifest_path);

    // Build every entry first, remembering (rather than propagating) the
    // first failure. The file is rewritten once, at the end, so bailing out
    // early would throw away entries that were perfectly fine to write and
    // leave the manifest describing a state that never existed.
    let mut build_error = None;
    for write in &plan.writes {
        if write.action == WriteAction::NoOp {
            continue;
        }
        let entry = match entry_for(store, agent, &write.name, &target.format) {
            Ok(entry) => entry,
            Err(e) => {
                build_error = Some(e);
                break;
            }
        };
        manifest.record(&write.name, TrackedContent::Whole, &manifest_key(&entry));
        object.insert(write.name.clone(), entry);
        report.applied += 1;
    }

    for name in &plan.removals {
        object.remove(name);
        manifest.forget(name);
        report.applied += 1;
    }

    // Only persist the manifest once the file it describes is actually on
    // disk: recording an entry that never landed would license deleting
    // someone else's. The reverse (a successful write whose manifest save
    // failed) merely leaves the entry untracked, which is the safe side.
    let base = scope_base(scope, project_root)?;
    write_managed_servers(&target, &object, &base)?;
    let saved = manifest.save(&manifest_path);
    match build_error {
        Some(e) => {
            if let Err(save_error) = saved {
                report
                    .warnings
                    .push(format!("could not record what was written: {save_error}"));
            }
            Err(e)
        }
        None => {
            saved?;
            Ok(report)
        }
    }
}

fn entry_for(
    store: &Store,
    agent: &dyn Agent,
    name: &str,
    format: &McpConfigFormat,
) -> Result<serde_json::Value> {
    let server = store.load_mcp_server(name)?;
    materialize_entry(&server, format)?.ok_or_else(|| Error::McpNoTransport {
        server: server.name.clone(),
        message: format!(
            "nothing {} can use — it needs {}",
            agent.display_name(),
            match format {
                McpConfigFormat::Json { .. } => "a `command` (stdio)",
                McpConfigFormat::TomlTables { .. } => "a `command` (stdio) or a `url` (http)",
            }
        ),
    })
}

/// Add the agent's project-scope MCP config file to `.gitignore` when — and
/// only when — this write could put a resolved credential in it.
///
/// Two things used to be wrong here, in opposite directions. It triggered on
/// *any* store server in the scope holding a secret, even when none of the
/// servers going into **this** file did, so a Copilot user got `.vscode/mcp.json`
/// gitignored (a file the Copilot docs tell people to commit) because some
/// unrelated Claude-only server referenced a token. And it appended to
/// `.gitignore` in directories that aren't git repositories at all, where the
/// file means nothing and shaic has no business creating one.
///
/// The safety property it exists for is unchanged: if the file will hold a
/// resolved secret and the project *is* a git repo, the ignore entry goes in
/// before the write, or the write doesn't happen.
fn ensure_gitignored_if_holds_secrets(
    store: &Store,
    agent: &dyn Agent,
    scope: Scope,
    relative_target: &Path,
    project_root: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let holds_secret = store.list_mcp_servers()?.0.into_iter().any(|s| {
        s.scope.contains(&scope)
            && s.agents.contains(&agent.id())
            && s.env.values().any(|v| matches!(v, EnvValue::Secret { .. }))
    });
    if !holds_secret {
        return Ok(());
    }
    if !project_root.join(".git").exists() {
        // Nothing to protect against: there is no repository here, so there
        // is no commit that could publish the file. Creating a `.gitignore`
        // anyway would just leave litter in someone's plain directory.
        warnings.push(format!(
            "{} can hold a resolved secret and {} is not a git repository — \
             wrote it without a .gitignore entry; add one if you later `git init` here",
            relative_target.display(),
            project_root.display()
        ));
        return Ok(());
    }

    let pattern = relative_target.to_string_lossy().replace('\\', "/");
    let gitignore_path = project_root.join(".gitignore");
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

fn manifest_key(entry: &serde_json::Value) -> String {
    let mut redacted = entry.clone();
    if let Some(obj) = redacted.as_object_mut() {
        obj.remove("env");
    }
    serde_json::to_string(&redacted).unwrap_or_default()
}

fn materialize_entry(
    server: &McpServer,
    format: &McpConfigFormat,
) -> Result<Option<serde_json::Value>> {
    match format {
        McpConfigFormat::Json { .. } => {
            if !server.has_stdio() {
                return Ok(None);
            }
            let resolved_env = resolve_env(server)?;
            Ok(Some(to_stdio_entry(server, &resolved_env)))
        }
        McpConfigFormat::TomlTables { .. } => {
            if server.has_http() {
                match resolve_bearer_env_var_name(server) {
                    Ok(bearer) => return Ok(Some(to_http_entry(server, bearer.as_deref()))),
                    // Dual-transport server: if the HTTP bearer isn't set yet,
                    // fall back to stdio rather than skipping Codex entirely.
                    Err(Error::SecretNotSet { .. }) if server.has_stdio() => {}
                    Err(e) => return Err(e),
                }
            }
            if server.has_stdio() {
                let resolved_env = resolve_env(server)?;
                return Ok(Some(to_stdio_entry(server, &resolved_env)));
            }
            Ok(None)
        }
    }
}

fn to_stdio_entry(
    server: &McpServer,
    resolved_env: &BTreeMap<String, String>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("command".to_string(), server.command.clone().into());
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

fn to_http_entry(server: &McpServer, bearer_env_var: Option<&str>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    if let Some(url) = &server.url {
        obj.insert("url".to_string(), url.clone().into());
    }
    if let Some(name) = bearer_env_var {
        obj.insert("bearer_token_env_var".to_string(), name.into());
    }
    obj.into()
}

fn server_from_on_disk_entry(
    name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
    existing: Option<&McpServer>,
    scope: Scope,
    pulled_by: AgentId,
) -> Result<McpServer> {
    let disk_command = obj
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let disk_has_stdio = !disk_command.is_empty();
    let disk_url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(String::from)
        .filter(|u| !u.is_empty());
    let disk_has_http = disk_url.is_some();

    // Merge transports: an agent that only writes HTTP (Codex) must not wipe
    // stdio fields the store already holds for Cursor/Claude, and vice versa.
    let (command, args, env) = if disk_has_stdio {
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
        let mut env = BTreeMap::new();
        if let Some(env_obj) = obj.get("env").and_then(|v| v.as_object()) {
            for (key, value) in env_obj {
                if let Some(on_disk) = value.as_str() {
                    let previous = existing.and_then(|s| s.env.get(key));
                    env.insert(key.clone(), reconciled_env_value(previous, on_disk));
                }
            }
        }
        (disk_command, args, env)
    } else if let Some(ex) = existing {
        (ex.command.clone(), ex.args.clone(), ex.env.clone())
    } else {
        (String::new(), Vec::new(), BTreeMap::new())
    };

    let (url, bearer_token_env_var) = if disk_has_http {
        let bearer = obj
            .get("bearer_token_env_var")
            .and_then(|v| v.as_str())
            .map(|on_disk| {
                reconciled_bearer_env_var(
                    existing.and_then(|s| s.bearer_token_env_var.as_ref()),
                    on_disk,
                )
            });
        (disk_url, bearer)
    } else if let Some(ex) = existing {
        (ex.url.clone(), ex.bearer_token_env_var.clone())
    } else {
        (None, None)
    };

    if url.is_none() && command.is_empty() {
        return Err(Error::McpNoTransport {
            server: name.to_string(),
            message: "the on-disk entry has neither a `command` (stdio) nor a `url` (http)"
                .to_string(),
        });
    }

    let mut scopes = existing.map(|s| s.scope.clone()).unwrap_or_default();
    if !scopes.contains(&scope) {
        scopes.push(scope);
    }

    // New pulls: stdio servers fan out to every agent (bidirectional sync).
    // HTTP-only servers are Codex-shaped — default to the agent that pulled
    // them so they are not pushed into JSON agents that cannot use them.
    let agents = existing.map(|s| s.agents.clone()).unwrap_or_else(|| {
        if disk_has_http && !disk_has_stdio {
            vec![pulled_by]
        } else {
            AgentId::ALL.to_vec()
        }
    });

    let server = McpServer {
        name: name.to_string(),
        command,
        args,
        env,
        url,
        bearer_token_env_var,
        scope: scopes,
        agents,
    };
    server.validate()?;
    Ok(server)
}

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

fn reconciled_bearer_env_var(previous: Option<&EnvValue>, on_disk: &str) -> EnvValue {
    if let Some(EnvValue::Secret { secret }) = previous
        && secret == on_disk
    {
        return EnvValue::Secret {
            secret: secret.clone(),
        };
    }
    EnvValue::Secret {
        secret: on_disk.to_string(),
    }
}

fn read_managed_servers(target: &McpTarget) -> Result<BTreeMap<String, serde_json::Value>> {
    match &target.format {
        McpConfigFormat::Json { servers_key } => {
            read_managed_json_object(&target.path, servers_key)
        }
        McpConfigFormat::TomlTables { table_prefix } => {
            read_toml_tables(&target.path, table_prefix)
        }
    }
}

fn write_managed_servers(
    target: &McpTarget,
    servers: &BTreeMap<String, serde_json::Value>,
    base: &Path,
) -> Result<()> {
    match &target.format {
        McpConfigFormat::Json { servers_key } => {
            write_managed_json_object(&target.path, servers_key, servers, base)
        }
        McpConfigFormat::TomlTables { table_prefix } => {
            write_toml_tables(&target.path, table_prefix, servers, base)
        }
    }
}

fn read_top_level_json(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
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

fn read_managed_json_object(path: &Path, key: &str) -> Result<BTreeMap<String, serde_json::Value>> {
    let top = read_top_level_json(path)?;
    match top.get(key) {
        Some(serde_json::Value::Object(inner)) => {
            Ok(inner.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }
        None => Ok(BTreeMap::new()),
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

fn write_managed_json_object(
    path: &Path,
    key: &str,
    object: &BTreeMap<String, serde_json::Value>,
    base: &Path,
) -> Result<()> {
    let target = path_guard::ensure_within(base, path)?;
    let mut top = read_top_level_json(&target)?;
    let inner: serde_json::Map<String, serde_json::Value> =
        object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    top.insert(key.to_string(), serde_json::Value::Object(inner));
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(top))
        .map_err(|e| Error::FrontmatterParse(e.to_string()))?;
    text.push('\n');
    writer::write_atomic(base, &target, &text, 0o600)
}

fn read_toml_document(path: &Path) -> Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(DocumentMut::new()),
        Ok(raw) => raw
            .parse::<DocumentMut>()
            .map_err(|e| Error::FrontmatterParse(format!("{}: invalid TOML: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_toml_tables(path: &Path, prefix: &str) -> Result<BTreeMap<String, serde_json::Value>> {
    let doc = read_toml_document(path)?;
    let Some(item) = doc.get(prefix) else {
        return Ok(BTreeMap::new());
    };
    let Some(table) = item.as_table() else {
        return Err(Error::FrontmatterParse(format!(
            "{}: {prefix:?} is not a TOML table",
            path.display()
        )));
    };
    let mut servers = BTreeMap::new();
    for (name, item) in table.iter() {
        if let Some(sub) = item.as_table() {
            servers.insert(name.to_string(), table_to_json(sub)?);
        }
    }
    Ok(servers)
}

fn write_toml_tables(
    path: &Path,
    prefix: &str,
    servers: &BTreeMap<String, serde_json::Value>,
    base: &Path,
) -> Result<()> {
    let target = path_guard::ensure_within(base, path)?;
    let mut doc = read_toml_document(&target)?;
    if doc.get(prefix).is_none() {
        let mut empty = Table::new();
        empty.set_implicit(false);
        doc.insert(prefix, Item::Table(empty));
    }
    let Some(prefix_item) = doc.get_mut(prefix) else {
        return Err(Error::FrontmatterParse(format!(
            "{}: missing {prefix:?} table after insert",
            path.display()
        )));
    };
    let Some(prefix_table) = prefix_item.as_table_mut() else {
        return Err(Error::FrontmatterParse(format!(
            "{}: {prefix:?} is not a TOML table",
            path.display()
        )));
    };

    let stale: Vec<String> = prefix_table
        .iter()
        .map(|(k, _)| k.to_string())
        .filter(|k| !servers.contains_key(k))
        .collect();
    for name in stale {
        prefix_table.remove(&name);
    }

    for (name, value) in servers {
        let existing = prefix_table.get(name).and_then(|i| i.as_table()).cloned();
        let merged = merge_managed_server_table(existing.as_ref(), value)?;
        prefix_table.insert(name, Item::Table(merged));
    }

    let mut text = doc.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    writer::write_atomic(base, &target, &text, 0o600)
}

/// Keys shaic owns inside an MCP server table. Unknown keys (`auth`,
/// `http_headers`, …) are preserved across updates.
const MANAGED_TOML_KEYS: &[&str] = &["command", "args", "env", "url", "bearer_token_env_var"];

fn merge_managed_server_table(
    existing: Option<&Table>,
    value: &serde_json::Value,
) -> Result<Table> {
    let mut table = existing.cloned().unwrap_or_else(Table::new);
    for key in MANAGED_TOML_KEYS {
        table.remove(key);
    }
    let obj = value.as_object().ok_or_else(|| {
        Error::FrontmatterParse("expected a JSON object for MCP server entry".to_string())
    })?;
    for (k, v) in obj {
        table.insert(k, json_to_toml_item(v));
    }
    Ok(table)
}

fn table_to_json(table: &Table) -> Result<serde_json::Value> {
    let mut map = serde_json::Map::new();
    for (k, v) in table.iter() {
        map.insert(k.to_string(), toml_item_to_json(v)?);
    }
    Ok(serde_json::Value::Object(map))
}

fn toml_item_to_json(item: &Item) -> Result<serde_json::Value> {
    match item {
        Item::Value(v) => Ok(toml_value_to_json(v)),
        Item::Table(t) => table_to_json(t),
        Item::ArrayOfTables(_) => Err(Error::FrontmatterParse(
            "array-of-tables not supported in MCP server entries".to_string(),
        )),
        Item::None => Ok(serde_json::Value::Null),
    }
}

fn toml_value_to_json(v: &TomlValue) -> serde_json::Value {
    match v {
        TomlValue::String(s) => s.value().clone().into(),
        TomlValue::Integer(i) => (*i.value()).into(),
        TomlValue::Float(f) => serde_json::Number::from_f64(*f.value())
            .map(Into::into)
            .unwrap_or(serde_json::Value::Null),
        TomlValue::Boolean(b) => (*b.value()).into(),
        TomlValue::Datetime(d) => d.to_string().into(),
        TomlValue::Array(a) => a
            .iter()
            .map(toml_value_to_json)
            .collect::<serde_json::Value>(),
        TomlValue::InlineTable(t) => {
            let map: serde_json::Map<String, serde_json::Value> = t
                .iter()
                .map(|(k, v)| (k.to_string(), toml_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

fn json_to_toml_item(value: &serde_json::Value) -> Item {
    match value {
        serde_json::Value::Null => Item::None,
        serde_json::Value::Bool(b) => Item::Value(TomlValue::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Item::Value(TomlValue::from(i))
            } else if let Some(f) = n.as_f64() {
                Item::Value(TomlValue::from(f))
            } else {
                Item::Value(TomlValue::from(n.to_string()))
            }
        }
        serde_json::Value::String(s) => Item::Value(TomlValue::from(s.as_str())),
        serde_json::Value::Array(a) => {
            let mut arr = Array::new();
            for v in a {
                if let Some(scalar) = json_scalar_to_toml(v) {
                    arr.push(scalar);
                }
            }
            Item::Value(TomlValue::Array(arr))
        }
        serde_json::Value::Object(o) => {
            let mut table = Table::new();
            for (k, v) in o {
                table.insert(k, json_to_toml_item(v));
            }
            Item::Table(table)
        }
    }
}

fn json_scalar_to_toml(value: &serde_json::Value) -> Option<TomlValue> {
    match value {
        serde_json::Value::String(s) => Some(TomlValue::from(s.as_str())),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(TomlValue::from)
            .or_else(|| n.as_f64().map(TomlValue::from)),
        serde_json::Value::Bool(b) => Some(TomlValue::from(*b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::codex::Codex;
    use crate::mcp::EnvValue;

    #[test]
    fn to_stdio_entry_omits_empty_args_and_env() {
        let server = McpServer::new(
            "bare".to_string(),
            "npx".to_string(),
            vec![],
            BTreeMap::new(),
            vec![Scope::Project],
        )
        .unwrap();
        let entry = to_stdio_entry(&server, &BTreeMap::new());
        let obj = entry.as_object().unwrap();
        assert_eq!(obj.get("command").unwrap(), "npx");
        assert!(!obj.contains_key("args"));
        assert!(!obj.contains_key("env"));
    }

    #[test]
    fn write_managed_json_object_preserves_unrelated_top_level_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"unrelatedSetting": true, "mcpServers": {}}"#).unwrap();

        let mut object = BTreeMap::new();
        object.insert("github".to_string(), serde_json::json!({"command": "npx"}));
        let target = McpTarget {
            path: path.clone(),
            format: McpConfigFormat::Json {
                servers_key: "mcpServers",
            },
        };
        write_managed_servers(&target, &object, dir.path()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["unrelatedSetting"], serde_json::json!(true));
        assert_eq!(value["mcpServers"]["github"]["command"], "npx");
    }

    #[test]
    fn read_managed_json_object_defaults_to_empty_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let target = McpTarget {
            path,
            format: McpConfigFormat::Json {
                servers_key: "mcpServers",
            },
        };
        let object = read_managed_servers(&target).unwrap();
        assert!(object.is_empty());
    }

    #[test]
    fn codex_toml_merge_preserves_unrelated_keys_and_extra_server_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "model = \"gpt-5\"\n\n[mcp_servers.github]\nurl = \"https://old.example/mcp/\"\nauth = \"oauth\"\n\n[mcp_servers.hand-written]\ncommand = \"manual\"\n",
        )
        .unwrap();

        let mut object = read_toml_tables(&path, "mcp_servers").unwrap();
        object.insert(
            "github".to_string(),
            serde_json::json!({
                "url": "https://api.githubcopilot.com/mcp/",
                "bearer_token_env_var": "GITHUB_PAT"
            }),
        );
        write_toml_tables(&path, "mcp_servers", &object, dir.path()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("model = \"gpt-5\""));
        assert!(raw.contains("[mcp_servers.hand-written]"));
        assert!(raw.contains("https://api.githubcopilot.com/mcp/"));
        assert!(raw.contains("GITHUB_PAT"));
        assert!(
            raw.contains("auth"),
            "unknown Codex keys must survive updates: {raw}"
        );
    }

    #[test]
    fn codex_http_entry_materializes_without_resolving_bearer_into_file() {
        let _guard = crate::security::secrets::ForceMissingSecrets::enable();
        let server = McpServer {
            name: "github".to_string(),
            command: String::new(),
            args: vec![],
            env: BTreeMap::new(),
            url: Some("https://api.githubcopilot.com/mcp/".to_string()),
            bearer_token_env_var: Some(EnvValue::Secret {
                secret: "GITHUB_PAT".to_string(),
            }),
            scope: vec![Scope::Global],
            agents: vec![crate::model::AgentId::Codex],
        };
        // Secret not set — should fail planning resolution
        let err = materialize_entry(
            &server,
            &McpConfigFormat::TomlTables {
                table_prefix: "mcp_servers",
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::SecretNotSet { .. }));
    }

    #[test]
    fn read_toml_tables_parses_codex_native_github_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[mcp_servers.github]
url = "https://api.githubcopilot.com/mcp/"
bearer_token_env_var = "GITHUB_PAT"
"#,
        )
        .unwrap();
        let servers = read_toml_tables(&path, "mcp_servers").unwrap();
        assert!(
            servers.contains_key("github"),
            "expected github subtable, got {servers:?}"
        );
    }

    #[test]
    fn codex_adapter_has_mcp_target() {
        let agent = Codex;
        let project = tempfile::tempdir().unwrap();
        let target = agent
            .mcp_target(Scope::Project, project.path())
            .expect("codex project mcp");
        assert!(target.path.ends_with(".codex/config.toml"));
        assert!(matches!(
            target.format,
            McpConfigFormat::TomlTables {
                table_prefix: "mcp_servers"
            }
        ));
    }
}
