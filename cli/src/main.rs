mod commands;
mod error;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use shaic_core::model::{AgentId, ItemKind};

#[derive(Parser)]
#[command(
    name = "shaic",
    version,
    about = "Sync AI-agent skills/rules/commands across agents via git"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create (or update the remote of) the canonical store.
    Init {
        #[arg(long)]
        remote: Option<String>,
    },
    /// Commit and push local store changes to the remote.
    Push {
        #[arg(long = "i-know-what-im-doing")]
        allow_secrets: bool,
    },
    /// Fetch and fast-forward-merge from the remote.
    Pull,
    /// Show store status and per-agent materialization drift.
    Status,
    /// Manage canonical skills/rules/commands.
    #[command(subcommand)]
    Item(ItemAction),
    /// Manage canonical MCP server definitions and their local secrets.
    #[command(subcommand)]
    Mcp(McpAction),
    /// Materialize canonical content into agent config locations.
    Sync {
        #[arg(long = "agent")]
        agents: Vec<AgentId>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Manage which project directories `shaic` syncs Project-scope content into.
    #[command(subcommand)]
    Project(ProjectAction),
    /// List known agents and discover their existing on-disk content.
    #[command(subcommand)]
    Agents(AgentsAction),
    /// Environment/health checks.
    Doctor,
    /// Launch the interactive TUI.
    Tui,
}

#[derive(Subcommand)]
enum ItemAction {
    /// Add a new skill, rule, or command (opens `$EDITOR`).
    Add {
        name: String,
        #[arg(long, default_value = "skill")]
        kind: ItemKind,
    },
    /// Edit an existing item (opens `$EDITOR`).
    Edit {
        name: String,
        #[arg(long, default_value = "skill")]
        kind: ItemKind,
    },
    /// Remove an item from the store.
    Rm {
        name: String,
        #[arg(long, default_value = "skill")]
        kind: ItemKind,
    },
    /// List items, optionally filtered by kind.
    List {
        #[arg(long)]
        kind: Option<ItemKind>,
    },
}

#[derive(Subcommand)]
enum McpAction {
    /// Add a new MCP server definition (opens `$EDITOR`). Real credentials
    /// belong in `env` as `{ secret = "NAME" }`, never as a literal value —
    /// set the value once per machine with `shaic mcp secret set`.
    Add { name: String },
    /// Edit an existing MCP server definition (opens `$EDITOR`).
    Edit { name: String },
    /// Remove an MCP server definition from the store.
    Rm { name: String },
    /// List MCP server definitions (never prints secret values).
    List,
    /// Manage this machine's local secret values for MCP server env vars —
    /// stored in the OS keychain, never written to the canonical store.
    #[command(subcommand)]
    Secret(McpSecretAction),
}

#[derive(Subcommand)]
enum McpSecretAction {
    /// Set (or overwrite) a secret's value on this machine. Prompts with
    /// input hidden; never accepts the value as a CLI argument.
    Set { name: String },
    /// Remove a secret from this machine's keychain.
    Rm { name: String },
    /// List secret names known on this machine (never their values).
    List,
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Register a project directory for Project-scope sync.
    Add { path: PathBuf },
    /// List registered project directories.
    List,
    /// Unregister a project directory.
    Rm { path: PathBuf },
}

#[derive(Subcommand)]
enum AgentsAction {
    /// List all known agents and their supported scopes/kinds.
    List,
    /// Show existing on-disk content for an agent (or all agents) that
    /// hasn't been imported into the store yet.
    Discover {
        #[arg(long)]
        agent: Option<AgentId>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        None | Some(Command::Tui) => commands::tui::run(),
        Some(Command::Init { remote }) => commands::init::run(remote),
        Some(Command::Push { allow_secrets }) => commands::push::run(allow_secrets),
        Some(Command::Pull) => commands::pull::run(),
        Some(Command::Status) => commands::status::run(),
        Some(Command::Item(action)) => commands::item::run(action),
        Some(Command::Mcp(action)) => commands::mcp::run(action),
        Some(Command::Sync {
            agents,
            global,
            project,
            all,
            dry_run,
            yes,
        }) => commands::sync::run(agents, global, project, all, dry_run, yes),
        Some(Command::Project(action)) => commands::project::run(action),
        Some(Command::Agents(action)) => commands::agents::run(action),
        Some(Command::Doctor) => commands::doctor::run(),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
