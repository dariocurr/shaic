mod commands;
mod error;
mod exit;
mod self_update;

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
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
        /// Replace an already-configured origin. Without this, re-running
        /// `init --remote` against a store that already has a different
        /// origin is refused.
        #[arg(long)]
        force: bool,
    },
    /// Commit and push local store changes to the remote.
    Push {
        #[arg(long = "i-know-what-im-doing")]
        allow_secrets: bool,
        #[arg(long)]
        json: bool,
    },
    /// Fetch and fast-forward-merge from the remote.
    Pull {
        #[arg(long = "i-know-what-im-doing")]
        allow_secrets: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show store status and per-agent materialization drift.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Manage canonical skills/rules/commands.
    #[command(subcommand)]
    Item(ItemAction),
    /// Manage canonical MCP server definitions and their local secrets.
    #[command(subcommand)]
    Mcp(McpAction),
    /// Materialize the store out to agent config files. Does not pull
    /// agent files into the store — use `shaic import` for that.
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
    /// Pull agent on-disk files into the canonical store. Does not write
    /// agent config — use `shaic sync` for that.
    Import {
        #[arg(long = "agent")]
        agents: Vec<AgentId>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: bool,
        #[arg(long)]
        all: bool,
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
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Check for or install GitHub Release updates.
    #[command(name = "self", subcommand)]
    SelfUpdate(SelfUpdateAction),
    /// Launch the interactive TUI.
    Tui,
}

#[derive(Subcommand)]
enum SelfUpdateAction {
    /// Print whether a newer release is available.
    Check,
    /// Download and install the latest release binary for this platform.
    Update {
        #[arg(long)]
        yes: bool,
    },
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
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| l.to_string()).unwrap_or_default();
        eprintln!(
            "error: shaic panicked ({loc}). this is a bug — report it at https://github.com/dariocurr/shaic/issues"
        );
    }));

    let cli = Cli::parse();
    let result = match cli.command {
        None => {
            if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                commands::tui::run()
            } else {
                let _ = Cli::command().print_help();
                Ok(())
            }
        }
        Some(Command::Tui) => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                Err(crate::error::CliError::Message(
                    "shaic tui needs a terminal — stdout is not a tty".to_string(),
                ))
            } else {
                commands::tui::run()
            }
        }
        Some(Command::Init { remote, force }) => commands::init::run(remote, force),
        Some(Command::Push {
            allow_secrets,
            json,
        }) => commands::push::run(allow_secrets, json),
        Some(Command::Pull {
            allow_secrets,
            json,
        }) => commands::pull::run(allow_secrets, json),
        Some(Command::Status { json }) => commands::status::run(json),
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
        Some(Command::Import {
            agents,
            global,
            project,
            all,
            yes,
        }) => commands::import::run(agents, global, project, all, yes),
        Some(Command::Project(action)) => commands::project::run(action),
        Some(Command::Agents(action)) => commands::agents::run(action),
        Some(Command::Doctor { json }) => commands::doctor::run(json),
        Some(Command::SelfUpdate(action)) => match action {
            SelfUpdateAction::Check => commands::self_cmd::run_check(),
            SelfUpdateAction::Update { yes } => commands::self_cmd::run_update(yes),
        },
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(exit::from_error(&err));
    }
}
