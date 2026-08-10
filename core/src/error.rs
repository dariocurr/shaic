use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("git command failed: {0}")]
    Git(String),

    #[error("store has diverged from origin — run `shaic pull` or resolve manually in {store}")]
    Diverged { store: PathBuf },

    #[error("uncommitted changes in {store} — commit or stash before pulling")]
    UncommittedChanges { store: PathBuf },

    #[error("remote url rejected: {0}")]
    InvalidRemote(String),

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

    #[error("secret store error: {0}")]
    Secret(String),

    #[error(
        "MCP server {server:?} references secret {secret:?}, which isn't set on this machine — run `shaic mcp secret set {secret}`"
    )]
    SecretNotSet { server: String, secret: String },
}

pub type Result<T> = std::result::Result<T, Error>;
