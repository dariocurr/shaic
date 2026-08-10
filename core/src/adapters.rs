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

/// Where (and under which top-level JSON key) an agent's MCP servers live
/// for one scope.
#[derive(Debug, Clone)]
pub struct McpTarget {
    pub path: PathBuf,
    pub servers_key: &'static str,
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

    /// The only trusted write boundary for this agent+scope.
    fn root(&self, scope: Scope, project_root: &Path) -> PathBuf;

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
    /// either the agent has no MCP support, or its config file mixes MCP
    /// servers with unrelated settings in a shape that isn't safe to
    /// blind-merge into (e.g. Claude Code's global `~/.claude.json`, Codex's
    /// `~/.codex/config.toml`) — those stay unsupported here even though the
    /// agent itself supports MCP, until that gets a real design. Independent
    /// of `supported_scopes()`, since an agent can support MCP in a
    /// different set of scopes than it supports skills/rules/commands in.
    fn mcp_target(&self, _scope: Scope, _project_root: &Path) -> Option<McpTarget> {
        None
    }
}

pub fn registry() -> Vec<Box<dyn Agent>> {
    vec![
        Box::new(claude_code::ClaudeCode),
        Box::new(cursor::Cursor),
        Box::new(windsurf::Windsurf),
        Box::new(copilot::Copilot),
        Box::new(codex::Codex),
        Box::new(gemini::Gemini),
        Box::new(google_antigravity::Antigravity),
        Box::new(cline::Cline),
    ]
}

pub fn by_id(id: AgentId) -> Box<dyn Agent> {
    registry()
        .into_iter()
        .find(|a| a.id() == id)
        .expect("registry() covers every AgentId variant")
}
