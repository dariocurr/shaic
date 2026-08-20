use std::path::{Path, PathBuf};

use crate::model::{AgentId, ContentForm, Item, ItemKind, Scope};

pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod common;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod google_antigravity;
pub mod opencode;
pub mod windsurf;

#[derive(Debug, Clone)]
pub struct RenderedFile {
    pub relative_path: PathBuf,
    pub contents: String,
    pub scope: Scope,
    pub form: ContentForm,
}

#[derive(Debug, Clone)]
pub struct DiscoveredContent {
    pub source_path: PathBuf,
    pub scope: Scope,
    pub raw: String,
    pub form: ContentForm,
}

/// Where (and in what on-disk shape) an agent's MCP servers live for one scope.
#[derive(Debug, Clone)]
pub struct McpTarget {
    pub path: PathBuf,
    pub format: McpConfigFormat,
}

/// On-disk shape for an agent's MCP config file.
#[derive(Debug, Clone)]
pub enum McpConfigFormat {
    /// Dedicated JSON file: `{ "mcpServers": { "name": { ... } } }`.
    Json { servers_key: &'static str },
    /// OpenCode shared settings: `{ "mcp": { "name": { "type": "local"|"remote", ... } } }`.
    /// Stdio → `type: local` with `command` as `[cmd, ...args]` and `environment`;
    /// HTTP → `type: remote` with `url` and optional Bearer via `{env:NAME}`.
    OpenCodeJson { servers_key: &'static str },
    /// Shared TOML settings file: `[mcp_servers.name]` tables (Codex).
    TomlTables { table_prefix: &'static str },
}

/// One adapter per supported agent. Every method is a pure function over its
/// inputs except `discover_existing`, which reads (never writes) the
/// filesystem to find content already on disk. `materialize::writer` is the
/// only code that ever writes into the path returned by `root`.
pub trait Agent: Send + Sync {
    fn id(&self) -> AgentId;
    fn display_name(&self) -> &'static str;
    fn supported_scopes(&self) -> &'static [Scope];
    fn supported_kinds(&self) -> &'static [ItemKind];

    /// The only trusted write boundary for this agent+scope, or `None` when
    /// there isn't one on this machine.
    ///
    /// `None` (rather than a guessed path) is what makes "shaic wrote my
    /// config into whatever directory I happened to be standing in"
    /// impossible: the global scope for several agents is rooted at the home
    /// directory, and `platform::home_dir()` can genuinely return nothing (no
    /// `$HOME`, no passwd entry). Falling back to `.` there meant a
    /// `.claude/`, `.codex/` or `.gemini/` tree appearing in the current
    /// working directory. Callers treat `None` as "skip this agent+scope",
    /// the same way `mcp_target` already treated it.
    fn root(&self, scope: Scope, project_root: &Path) -> Option<PathBuf>;

    /// Render one kind's worth of items for one scope. For `ContentForm::SingleFile`,
    /// `RenderedFile::contents` is the *inner region* only — splicing it between
    /// markers in whatever the file already contains is `materialize::writer`'s job,
    /// not this pure function's.
    fn render(
        &self,
        kind: ItemKind,
        items: &[Item],
        scope: Scope,
        existing_form: Option<ContentForm>,
    ) -> Vec<RenderedFile>;

    fn discover_existing(
        &self,
        kind: ItemKind,
        scope: Scope,
        project_root: &Path,
    ) -> Vec<DiscoveredContent>;

    /// Parse this agent's on-disk `kind`+`scope` content back into canonical
    /// items — the inverse of `render`, best-effort. Used to pull an agent's
    /// hand-edited or hand-added content back into the store before pushing
    /// the store's state back out to every agent, the same way MCP servers
    /// reconcile. Returns an empty vec (the default) for a kind this agent's
    /// on-disk format can't be reversed for — that content still gets into
    /// the store fine via `shaic item add`/`edit`, just not automatically
    /// from this agent's own files.
    fn reconcile_existing(&self, kind: ItemKind, scope: Scope, project_root: &Path) -> Vec<Item> {
        let _ = (kind, scope, project_root);
        Vec::new()
    }

    /// True only for agents whose on-disk convention isn't confirmed yet
    /// (currently just Antigravity). `materialize` refuses to write for these
    /// regardless of what `render`/`root` would otherwise produce.
    fn experimental_read_only(&self) -> bool {
        false
    }

    /// Where this agent's MCP servers live for `scope`, if shaic can safely
    /// write there. `None` (the default) means "not supported for MCP" —
    /// either the agent has no MCP support, or its config shape isn't safe
    /// to merge into yet. Override with `McpConfigFormat::Json` for a
    /// dedicated MCP file, `McpConfigFormat::OpenCodeJson` for OpenCode's
    /// `mcp` object shape, or `McpConfigFormat::TomlTables` for a shared
    /// settings file where only `[mcp_servers.*]` is rewritten (Codex).
    /// Independent of `supported_scopes()`.
    fn mcp_target(&self, _scope: Scope, _project_root: &Path) -> Option<McpTarget> {
        None
    }
}

/// Every adapter, in display order. Every implementor is a unit struct with
/// no state, so one shared `&'static` reference each is all anyone ever needs
/// — the whole set used to be heap-allocated on every lookup (see `by_id`).
static REGISTRY: &[&dyn Agent] = &[
    &claude_code::ClaudeCode,
    &cursor::Cursor,
    &windsurf::Windsurf,
    &copilot::Copilot,
    &codex::Codex,
    &opencode::OpenCode,
    &gemini::Gemini,
    &google_antigravity::Antigravity,
    &cline::Cline,
];

pub fn registry() -> &'static [&'static dyn Agent] {
    REGISTRY
}

/// The adapter for one `AgentId`.
///
/// A total `match` rather than a search through `registry()`: it costs
/// nothing (this is called inside nested agent x scope x kind loops, where it
/// used to box all adapters just to hand back one), and it removes the
/// `expect("registry() covers every AgentId variant")` that stood in for what
/// the compiler can check — adding an `AgentId` variant without an adapter is
/// now a build error, not a panic at runtime.
pub fn by_id(id: AgentId) -> &'static dyn Agent {
    match id {
        AgentId::ClaudeCode => &claude_code::ClaudeCode,
        AgentId::Cursor => &cursor::Cursor,
        AgentId::Windsurf => &windsurf::Windsurf,
        AgentId::Copilot => &copilot::Copilot,
        AgentId::Codex => &codex::Codex,
        AgentId::OpenCode => &opencode::OpenCode,
        AgentId::Gemini => &gemini::Gemini,
        AgentId::Antigravity => &google_antigravity::Antigravity,
        AgentId::Cline => &cline::Cline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_id_and_registry_cover_exactly_every_agent_id() {
        let registered: Vec<AgentId> = registry().iter().map(|a| a.id()).collect();
        assert_eq!(
            registered,
            AgentId::ALL.to_vec(),
            "registry() must list every agent exactly once, in AgentId order"
        );
        for id in AgentId::ALL {
            assert_eq!(
                by_id(id).id(),
                id,
                "by_id must hand back the adapter it was asked for"
            );
        }
    }
}
