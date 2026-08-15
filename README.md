# shaic

[![CI](https://github.com/dariocurr/shaic/actions/workflows/ci.yml/badge.svg)](https://github.com/dariocurr/shaic/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](#install)

**Switch agents. Keep your skills.**

*SHare AI Config — skills, hooks, agents, instructions, commands.*

You moved from Claude Code to Cursor. Every skill, rule, slash command, and MCP
server now has to be retyped by hand, in a different format. Then again for
Codex. Then again on the next laptop.

shaic is one git repo. `shaic sync` translates it into whatever format each
agent actually expects. No extra service. No merge engine. Just git. macOS,
Linux, and Windows.

```text
before                                      after
.claude/skills/*/SKILL.md                   ~/.shaic/store   ← git
.cursor/rules/*.mdc                               │
AGENTS.md                                         │  shaic sync
.github/copilot-instructions.md                   ▼
~/.claude.json  (mcpServers)                Claude  Cursor  Codex
.cursor/mcp.json                            Copilot Windsurf Gemini
~/.codex/config.toml                        Cline   …
```

Edit in Cursor, then `shaic import`. `shaic sync` writes every agent
shaic can materialize. Credential *values* never touch git — OS keychain,
per machine — but stdio secrets *are* written into that machine's agent
config files at sync time.

```sh
git clone https://github.com/dariocurr/shaic.git && cd shaic
cargo install --path cli

shaic init --remote git@github.com:you/your-shaic-store.git
shaic item add code-review-checklist
shaic sync
shaic push
```

Or drive it from the dashboard — `shaic tui`:

```text
⟡ shaic  ›  dashboard
╭─agents (↑/↓ select, Enter=detail)──────────────────────────╮
│ agent                     status                           │
│ Claude Code               ● in-sync                        │
│ Cursor                    ▲ drift                          │
│ Windsurf                  ● in-sync                        │
│ GitHub Copilot            ● in-sync                        │
│ OpenAI Codex CLI          ▲ drift                          │
│ Google Gemini CLI         ● in-sync                        │
│ Cline                     ✕ error                          │
│ Google Antigravity        ◐ experimental, read-only        │
╰────────────────────────────────────────────────────────────╯
╭────────────────────────────────────────────────────────────╮
│ pushed 3 items to Cursor                                   │
│    [p=push u=pull s=browse skill  i=setup  ?=help  q=quit] │
╰────────────────────────────────────────────────────────────╯
```

<details>
<summary>Contents</summary>

- [Why](#why)
- [Install](#install)
- [Quickstart](#quickstart)
- [Supported agents](#supported-agents)
- [Commands](#commands)
- [MCP servers and credentials](#mcp-servers-and-credentials)
- [How sync actually works](#how-sync-actually-works)
- [Security](#security)
- [Contributing](#contributing)

</details>

---

## Why

Each agent has its own files, its own frontmatter, its own idea of what a "rule"
even is. shaic is the source of truth. Agents are just render targets.

| You write once | shaic writes everywhere |
| --- | --- |
| Skills, rules, slash commands | Claude, Cursor, Windsurf, Copilot, Codex, Gemini, Cline |
| MCP server defs (`command`, `args`, `url`) | Claude, Cursor, Windsurf, Copilot, Codex — not Gemini, Cline, or Antigravity |
| Secrets | OS keychain, per machine. Stdio `env` values are resolved into the agent config file at sync; they never go in git. |

---

## Install

macOS and Linux (Homebrew):

```sh
brew install dariocurr/tap/shaic
```

Windows and anyone without Homebrew: GitHub Release binaries, or
`cargo install shaic`.

```sh
# macOS / Linux (no Homebrew):
curl -sSfL \
  https://raw.githubusercontent.com/dariocurr/shaic/main/scripts/install.sh \
  | sh
# Windows (PowerShell):
irm https://raw.githubusercontent.com/dariocurr/shaic/main/scripts/install.ps1 |
  iex

# or crates.io
cargo install shaic

# update an installed binary
shaic self check
shaic self update
```

Needs `git` on `PATH`. macOS, Linux, and Windows (x64 and ARM64).
Rust 1.85+ only if you install from source.

---

## Quickstart

```sh
# 1. Point shaic at a git remote you control
shaic init --remote git@github.com:you/your-shaic-store.git

# 2. Add content (opens $EDITOR). Skill is the default kind.
shaic item add code-review-checklist
shaic item add no-any-in-ts --kind rule
shaic item add release-checklist --kind command

# 3. Opt this directory in for project-scoped writes
shaic project add .

# 4. Preview, then materialize
shaic sync --agent claude-code --project --dry-run
shaic sync --agent claude-code --project

# 5. Ship the store to other machines
shaic push
```

On another machine: `shaic init --remote <same url>` clones the store, then
`shaic sync` materializes it locally.

Run `shaic` with no arguments (interactive terminal) or `shaic tui` for the
dashboard.

---

## Supported agents

| Agent | Convention | Scopes |
| --- | --- | --- |
| Claude Code | `.claude/` (`CLAUDE.md`, `skills/*/SKILL.md`, `commands/*.md`) | global + project |
| Cursor | `.cursor/rules/*.mdc` (or legacy `.cursorrules`) | project |
| Windsurf | `.windsurf/rules/*.md` + `workflows/*.md` (or legacy `.windsurfrules`) | project |
| GitHub Copilot | `.github/copilot-instructions.md`, `.github/instructions/*.instructions.md`, `.github/prompts/*.prompt.md` | project |
| OpenAI Codex CLI | `AGENTS.md`; skills at `~/.codex/skills/` (global) and `.codex/skills/` (project) | project (+ best-effort `~/.codex/`) |
| Google Gemini CLI | `GEMINI.md` | global + project |
| Cline | `.clinerules/` (or legacy single `.clinerules` file) | project |
| Google Antigravity | *experimental, read-only* — `.antigravity/rules/*.md` is discovered but never written; the write convention is unconfirmed | project |

### MCP write support

Only agents whose MCP config shaic can safely merge into:

| Agent | MCP file | Scopes |
| --- | --- | --- |
| Claude Code | `.mcp.json` (project), `~/.claude.json` (global) | global + project |
| Cursor | `.cursor/mcp.json` / `~/.cursor/mcp.json` | global + project |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | global only |
| GitHub Copilot (VS Code) | `.vscode/mcp.json` (`servers` key, not `mcpServers`) | project only |
| Codex CLI | `~/.codex/config.toml` / `.codex/config.toml` | global + project |

Gemini CLI, Cline, and Antigravity are not write-supported for MCP yet.

---

## Commands

| Command | What it does |
| --- | --- |
| `shaic init [--remote <url>] [--force]` | Create the canonical store. `--force` is required to replace an origin that is already set. |
| `shaic push` / `shaic pull` | Sync the store with its remote (fetch + fast-forward-only merge). `pull` needs a clean working tree. Both scan for obvious secrets; pass `--i-know-what-im-doing` to override. Add `--json` for machine-readable output. |
| `shaic status [--json]` | Store + per-agent drift at a glance |
| `shaic item add\|edit\|rm\|list --kind <skill\|rule\|command>` | Manage canonical items (skill is the default) |
| `shaic mcp add\|edit\|rm\|list` | Manage canonical MCP server definitions |
| `shaic mcp secret set\|rm\|list` | This machine's local secret values for MCP env vars |
| `shaic sync [--agent <id>]… [--global] [--project] [--all] [--dry-run] [--yes]` | Write the store out to agent config. Non-tty runs need `--yes`. |
| `shaic import [--agent <id>]… [--global] [--project] [--all] [--yes]` | Pull agent on-disk files into the store. Does not write agent files. |
| `shaic project add\|list\|rm` | Opt a directory into project-scoped writes |
| `shaic agents list\|discover` | Supported agents / find existing hand-written configs |
| `shaic doctor [--json]` | Environment and store health checks |
| `shaic self check\|update [--yes]` | Check for or install GitHub Release updates |
| `shaic tui` | Interactive dashboard (stdout must be a tty) |

`push` / `pull` are a plain fetch + fast-forward-only merge. Diverged history
fails loudly and points you at plain `git` in the store directory. There is no
custom conflict resolution. `pull` also refuses if the store has uncommitted
changes — commit or stash first.

### Exit codes

For scripts and fleet checks: `0` ok, `1` general failure, `2` usage error,
`3` git (diverged / network), `4` secret scan blocked, `5` config or store
not ready. See `CONTRIBUTING.md` for details.

---

## MCP servers and credentials

MCP *definitions* (`command`, `args`, non-secret `env`, `url`) sync via git like
everything else. Credential *values* never do — set them once per machine with
`shaic mcp secret set` (OS keychain) and they resolve locally at sync time.

Two transports live in the same canonical model. Agents pick what they can use:

| Transport | Fields | Typical agents | Credential handling |
| --- | --- | --- | --- |
| **stdio** | `command`, `args`, `env` | Cursor, Claude Code, Windsurf, Copilot | `{ secret = "NAME" }` in `env` → value resolved into the agent config file |
| **HTTP** | `url`, `bearer_token_env_var` | Codex | shaic writes the env var *name* into `config.toml`; Codex reads the value from the process environment at launch |

```sh
shaic mcp add my-server          # opens $EDITOR with a TOML template
shaic mcp secret set API_TOKEN   # prompts once, on this machine — value never synced
shaic sync --agent cursor --project
```

### Stdio

```toml
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
scope = ["project"]

[env]
LOG_LEVEL = "debug"                           # plain strings are fine
API_TOKEN = { secret = "API_TOKEN" }          # resolved locally at sync time
```

### HTTP (Codex)

Hosted MCP endpoints authenticate with a bearer token. shaic never writes the
token into `config.toml` — only the name of the env var Codex should read:

```toml
name = "remote-tools"
url = "https://mcp.example.com/"
bearer_token_env_var = { secret = "MCP_BEARER_TOKEN" }
scope = ["global"]
agents = ["codex"]
```

On each machine:

```sh
shaic mcp secret set MCP_BEARER_TOKEN   # paste once into the OS keychain
export MCP_BEARER_TOKEN=...             # Codex reads this at launch
shaic sync --agent codex
```

When `bearer_token_env_var = { secret = "NAME" }`, sync also checks that secret
is set in the keychain. That is a guardrail, not a substitute for exporting
`NAME` into the shell before starting Codex.

### Dual transport

One store entry can hold both stdio and HTTP fields. Each agent materializes the
transport it supports; `shaic import` **merges** so a Codex import does not
wipe stdio (or the reverse). Prefer `agents = ["codex"]` for HTTP-only servers
so they are not pushed to JSON agents that cannot use them.

`shaic mcp secret set <name>` stores the real value in this machine's OS
keychain (macOS Keychain / Linux Secret Service) — never in the git-tracked
store, never pushed, never pulled. A secret has to be set on *every* machine
that materializes that server. `shaic mcp secret list` only ever shows names;
nothing shaic prints, logs, or writes ever includes a resolved value except the
one real agent config file it's materialized into.

---

## How sync actually works

`shaic sync` is one-way: store → agents. It never writes the store.

`shaic import` is the other direction: agent files → store. It never writes
agent config. Hand-edit a Cursor rule, `shaic import`, `shaic sync`, and
Codex gets it.

If two agents disagree about the same item or server name, **the last
`import` wins**. There is no merge. Import one agent at a time
(`--agent cursor`) when you care which copy lands.

**Import is lossy.** Agent files are not the canonical format. Fields the
agent never stored (`description` on most non-Claude items, Skill vs Rule
on Cursor/Windsurf/Cline) cannot come back. `shaic sync` from the store is
the lossless direction. Treat `import` as a one-time bootstrap, not a
round-trip.

On another machine: `shaic init --remote <same url>` clones the store, then
`shaic sync` materializes it locally. Do not `import` there first unless you
intend to overlay that machine's existing agent files onto the store.

<details>
<summary>Item reversal is best-effort and per-adapter</summary>

Claude Code's own Skill/Command files already match the canonical frontmatter
shape exactly (lossless). Every other agent's on-disk format is agent-native
(Cursor's `globs`/`alwaysApply`, Copilot's `applyTo`, plain `# heading` sections
for the rest) and gets translated back field-by-field, dropping whatever that
format never encoded in the first place (mostly `description` for anything that
isn't Claude or a Command).

Cursor, Windsurf, and Cline render Skill and Rule into the exact same on-disk
files with no way to tell which was meant — reversal only ever reads those back
as Rule, never Skill, to avoid importing the same content twice under two
different kinds.
</details>

<details>
<summary>MCP merge targets that share a settings file</summary>

Claude Code's Global scope merges into `~/.claude.json` — the same file that
holds this machine's Claude Code auth/session/project state, not a dedicated MCP
file. The merge only ever touches the top-level `mcpServers` key (every other
key round-trips untouched), but a bug there has a bigger blast radius than any
other MCP target. If that trade-off makes you uneasy, manage Claude Code's
global MCP servers by hand instead.

Codex's `config.toml` is a large shared settings file (model, plugins, hooks,
…). The merge only rewrites `[mcp_servers.*]` tables shaic manages; top-level
keys outside that prefix round-trip, and unknown keys inside a managed server
table (e.g. `http_headers`) are preserved across updates.
</details>

<details>
<summary>Hand-typed credentials in agent config</summary>

A literal credential in an `env` value is indistinguishable from a real setting
once it's plain text in an agent's config, so it gets pulled into the store as a
literal too. `Store::save_mcp_server`'s secret-scan tripwire still blocks the
obviously-shaped ones (AWS keys, GitHub tokens, private key headers, …), but it
is not a guarantee. If you're not comfortable with that trade-off, don't
hand-edit `env` values with real credentials — use `shaic mcp secret set`
instead.
</details>

---

## Security

See [SECURITY.md](SECURITY.md) to report a vulnerability.

- **Git credentials are never stored by shaic.** The only remote thing persisted
  is the remote URL (validated to reject one with embedded userinfo, e.g.
  `https://user:token@...`). All git auth is delegated to whatever credential
  helper, SSH agent, or `~/.netrc` your system already has. shaic shells out to
  the real `git` binary rather than reimplementing auth.
- **MCP credential values** live in this machine's OS keychain via
  `shaic mcp secret set` — never in the git-tracked store. They are resolved
  into agent config files at sync time for stdio `env` values; for Codex HTTP,
  only the env var *name* is written.
- Config lives in the OS config dir, mode `0600` on Unix:
  - Linux: `$XDG_CONFIG_HOME/shaic/config.toml`
    (default `~/.config/shaic/config.toml`)
  - macOS: `~/Library/Application Support/shaic/config.toml`
  - Windows: `%APPDATA%\shaic\config.toml`
  An empty `enabled_agents` list (the default) means every known agent;
  otherwise only the listed ids are synced.
- Every write into an agent's directory is validated against path traversal and
  symlink escapes before it happens; single-file agents (`CLAUDE.md`,
  `AGENTS.md`, …) are only ever touched inside a delimited managed region, never
  overwritten wholesale.
- `shaic push` and `shaic pull` run a secret-scan tripwire over what would be
  committed or fast-forwarded — this also covers MCP server definitions, as a
  backstop against a literal credential accidentally being pasted into an
  `env` value instead of a `{ secret = "..." }` reference. The scan is
  best-effort, not a guarantee.
- `command` / `args` for MCP servers *do* sync via git like any other content —
  trust your remote the same way you already trust it for skill/rule bodies.

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT

<!-- Table rows cannot be wrapped, and GitHub needs raw HTML for collapsibles. -->
<!-- markdownlint-configure-file {
  "MD013": { "tables": false },
  "MD033": { "allowed_elements": ["details", "summary"] }
} -->
