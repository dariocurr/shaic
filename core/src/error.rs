use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A `git` *subprocess* failed — nothing else. Everything that used to be
    /// funnelled through here (directory walks, write targets without a
    /// parent, ...) now has its own variant, so "git is broken" and "this file
    /// is broken" stop looking identical to the user.
    #[error("git command failed: {0}")]
    Git(String),

    /// Walking a directory tree failed (permissions, a vanished directory,
    /// a symlink loop). Distinct from `Io` because walkdir's error carries its
    /// own context and isn't always backed by a `std::io::Error`.
    #[error("failed to walk {path}: {message}")]
    WalkDir { path: PathBuf, message: String },

    /// A JSON config file (an agent's `.mcp.json`, `settings.json`, ...) could
    /// not be parsed. Carries the path because these files are edited by hand
    /// and by other tools, so "which file" is the first thing the user needs.
    #[error("invalid JSON in {path}: {message}")]
    Json { path: PathBuf, message: String },

    /// As `Json`, for TOML config files (Codex's `config.toml`, the store's
    /// `mcp/*.toml`, `.shaic-store.toml`).
    #[error("invalid TOML in {path}: {message}")]
    Toml { path: PathBuf, message: String },

    /// The file parsed, but a key shaic needs to descend into holds the wrong
    /// kind of value (`"mcpServers": "yes"` instead of an object). Separate
    /// from `Json`/`Toml` so shaic never silently overwrites a structure it
    /// didn't understand.
    #[error("unexpected shape in {path}: {message}")]
    InvalidConfigShape { path: PathBuf, message: String },

    /// A path shaic must write to has no parent directory to create, which
    /// means the path itself is malformed rather than the filesystem failing.
    #[error("path {0} has no parent directory")]
    NoParentDirectory(PathBuf),

    /// A project-scoped operation resolved to a location outside the project
    /// it was invoked for. Sibling of `PathEscape`, kept separate because the
    /// boundary being defended is the project, not an agent's config root.
    #[error("refusing to touch {path}: it is outside the project root {project_root}")]
    OutsideProjectRoot {
        path: PathBuf,
        project_root: PathBuf,
    },

    #[error(
        "store has diverged from origin — shaic will not auto-merge; resolve in {store} with plain git (fetch, rebase or reset), then `shaic push`"
    )]
    Diverged { store: PathBuf },

    #[error("uncommitted changes in {store} — commit or stash before pulling")]
    UncommittedChanges { store: PathBuf },

    #[error("remote url rejected: {0}")]
    InvalidRemote(String),

    #[error("store already points at {current:?}; pass --force to replace it with {requested:?}")]
    RemoteAlreadySet { current: String, requested: String },

    #[error("invalid item name {0:?}: names must be a single path component with no separators")]
    InvalidName(String),

    #[error("refusing to write outside agent root: {candidate} escapes {root}")]
    PathEscape { root: PathBuf, candidate: PathBuf },

    #[error("refusing to write through symlink at {0}")]
    SymlinkEscape(PathBuf),

    #[error("frontmatter exceeds max size ({size} > {max} bytes)")]
    FrontmatterTooLarge { size: usize, max: usize },

    #[error("frontmatter contains disallowed YAML anchor/alias/merge key")]
    FrontmatterAnchorsRejected,

    #[error("frontmatter parse error: {0}")]
    FrontmatterParse(String),

    #[error("potential secret detected: {0} — pass --i-know-what-im-doing to override")]
    SecretDetected(String),

    #[error(
        "store schema version {found} is newer than this shaic build understands ({supported}) — upgrade shaic"
    )]
    SchemaTooNew { found: u32, supported: u32 },

    #[error("store not initialized — run `shaic init` first")]
    StoreNotInitialized,

    #[error("config error: {0}")]
    Config(String),

    #[error("MCP server {server:?} has no transport this agent can use: {message}")]
    McpNoTransport { server: String, message: String },

    #[error("secret store error: {0}")]
    Secret(String),

    #[error(
        "MCP server {server:?} references secret {secret:?}, which isn't set on this machine — run `shaic mcp secret set {secret}`"
    )]
    SecretNotSet { server: String, secret: String },
}

pub type Result<T> = std::result::Result<T, Error>;
