# shaic

Sync the skills, rules, commands, and MCP server definitions that steer your AI coding agents — across every agent installed on a machine, and across machines — using git as the only sync mechanism.

## What it does

You maintain one canonical set of skills/rules/commands in a local git repo (`~/.shaic/store`). `shaic` translates that content into the on-disk format each agent actually expects, and `push`/`pull` keep the canonical store in sync with a remote you control. There is no custom merge/conflict resolution: `push`/`pull` are a plain fetch + fast-forward-only merge, and anything else (a diverged history) fails loudly and points you at plain `git` in the store directory.

Sync is **bidirectional** for both items and MCP servers (see below): edit or add a skill/rule/command/MCP server directly in one agent's own config — no `shaic item add`/`mcp add` involved — and every real (non-`--dry-run`) `sync` pulls it into the store first, then pushes the store's merged state out to every other agent. There's no conflict resolution here either: if two agents disagree about the same item/server, whichever is processed last in that `sync` run wins.

Item reversal is necessarily best-effort and per-adapter: Claude Code's own Skill/Command files already match the canonical frontmatter shape exactly (lossless); every other agent's on-disk format is agent-native (Cursor's `globs`/`alwaysApply`, Copilot's `applyTo`, plain `# heading` sections for the rest) and gets translated back field-by-field, dropping whatever that format never encoded in the first place (mostly `description` for anything that isn't Claude or a Command). Cursor, Windsurf, and Cline render Skill and Rule into the exact same on-disk files with no way to tell which was meant — reversal only ever reads those back as Rule, never Skill, to avoid importing the same content twice under two different kinds.

### Supported agents

| Agent | Convention | Scopes |
| --- | --- | --- |
| Claude Code | `.claude/` (`CLAUDE.md`, `skills/*/SKILL.md`, `commands/*.md`) | global + project |
| Cursor | `.cursor/rules/*.mdc` (or legacy `.cursorrules`) | project |
| Windsurf | `.windsurf/rules/*.md` + `workflows/*.md` (or legacy `.windsurfrules`) | project |
| GitHub Copilot | `.github/copilot-instructions.md`, `.github/instructions/*.instructions.md`, `.github/prompts/*.prompt.md` | project |
| OpenAI Codex CLI | `AGENTS.md` | project (+ best-effort `~/.codex/`) |
| Google Gemini CLI | `GEMINI.md` | global + project |
| Google Antigravity | *experimental, read-only* — on-disk convention unconfirmed | project |
| Cline | `.clinerules/` (or legacy single `.clinerules` file) | project |

### MCP servers

MCP server *definitions* (`command`, `args`, non-secret `env` values) sync via git like everything else. Credential *values* never do — they're set once per machine (`shaic mcp secret set`, stored in the OS keychain) and resolved locally at sync time. See [MCP servers and credentials](#mcp-servers-and-credentials) below.

Sync is **bidirectional**: every real (non-`--dry-run`) `sync` first pulls each agent's on-disk MCP servers back into the canonical store, then pushes the store's (now-merged) state out to every agent. Add or edit a server directly in one agent's own config — no `shaic mcp add`/`edit` involved — and it reaches every other agent on the same `sync` run, same as if you'd added it through shaic itself. There's no conflict resolution: if two agents disagree about the same server name, whichever is processed last in that `sync` run wins. A hand-typed literal credential in an `env` value is indistinguishable from a real setting once it's plain text in an agent's config, so it gets pulled into the store as a literal too — `Store::save_mcp_server`'s secret-scan tripwire still blocks the obviously-shaped ones (AWS keys, GitHub tokens, private key headers, ...), but it is not a guarantee. If you're not comfortable with that trade-off, don't hand-edit `env` values with real credentials in them — use `shaic mcp secret set` instead.

Only agents with a config file dedicated solely to MCP servers (never mixed with unrelated settings) are currently write-supported, since materializing means merging into that file's managed key and leaving everything else in it untouched:

| Agent | MCP file | Scopes |
| --- | --- | --- |
| Claude Code | `.mcp.json` (project), `~/.claude.json` (global) | global + project |
| Cursor | `.cursor/mcp.json` / `~/.cursor/mcp.json` | global + project |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | global only |
| GitHub Copilot (VS Code) | `.vscode/mcp.json` | project only |

Claude Code's Global scope merges into `~/.claude.json` — the same file that holds this machine's Claude Code auth/session/project state, not a dedicated MCP file. The merge only ever touches the top-level `mcpServers` key (every other key round-trips untouched — see `write_managed_object_preserves_unrelated_top_level_keys`), but a bug there has a bigger blast radius than any other MCP target in this crate. If that trade-off makes you uneasy, manage Claude Code's global MCP servers by hand instead.

Codex CLI, Gemini CLI, Cline, and Antigravity aren't write-supported yet — their MCP config is either a large shared settings file (Codex's `config.toml`, Gemini's `settings.json`) or a path shaic isn't confident enough in yet (Cline, Antigravity) to write into safely.

## Install

Requires Rust (edition 2024) and a `git` binary on `PATH`.

```sh
cargo build --release
./target/release/shaic --help
```

## Quickstart

```sh
shaic init --remote git@github.com:you/your-shaic-store.git
shaic item add code-review-checklist                      # opens $EDITOR — a skill by default
shaic item add no-any-in-ts --kind rule                   # a rule instead
shaic item add release-checklist --kind command           # a slash command instead
shaic project add .                             # opt in this directory for project-scoped writes
shaic sync --agent claude-code --project --dry-run
shaic sync --agent claude-code --project
shaic push
```

On another machine: `shaic init --remote <same url>` clones the store, then `shaic sync` materializes it locally.

Run `shaic` with no arguments (in an interactive terminal) or `shaic tui` for the interactive dashboard.

## Commands

```text
shaic init [--remote <url>]     create or re-point the canonical store
shaic push / pull               sync the store with its remote (fetch + ff-only merge only)
shaic status                    store + per-agent drift at a glance
shaic item add|edit|rm|list --kind <skill|rule|command>   manage canonical items (skill is the default kind)
shaic mcp add|edit|rm|list      manage canonical MCP server definitions
shaic mcp secret set|rm|list    manage this machine's local secret values for MCP env vars
shaic sync                      materialize canonical content (items + MCP servers) into agent config
shaic project add|list|rm       opt a directory into project-scoped writes
shaic agents list|discover      supported agents / find existing hand-written configs
shaic doctor                    environment and store health checks
shaic tui                       interactive dashboard
```

## MCP servers and credentials

```sh
shaic mcp add github        # opens $EDITOR with a TOML template
shaic mcp secret set GITHUB_TOKEN    # prompts once, on this machine — value never synced
shaic sync --agent cursor --project
```

The store only ever holds a *reference* to a secret, never its value:

```toml
name = "github"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[env]
GITHUB_TOKEN = { secret = "GITHUB_TOKEN" }   # resolved locally at sync time
LOG_LEVEL = "debug"                           # non-secret values are fine as plain strings
```

`shaic mcp secret set <name>` stores the real value in this machine's OS keychain (macOS Keychain / Linux Secret Service) — never in the git-tracked store, never pushed, never pulled. Because of that, a secret has to be set on *every* machine that materializes that server (there's no other channel to move it through — the whole design here is that a credential value never travels over the same git connection everything else does). `shaic mcp secret list` only ever shows names; nothing shaic prints, logs, or writes ever includes a resolved value except the one real agent config file it's materialized into.

## Security

- **Credentials are never stored by shaic.** The only thing persisted is the remote URL (validated to reject one with embedded userinfo, e.g. `https://user:token@...`). All git auth is delegated to whatever credential helper, SSH agent, or `~/.netrc` your system already has configured, because shaic shells out to the real `git` binary rather than reimplementing auth.
- Config lives at `$XDG_CONFIG_HOME/shaic/config.toml` (`~/.config/shaic/config.toml`), `0600` on Unix.
- Every write into an agent's directory is validated against path traversal and symlink escapes before it happens; single-file agents (`CLAUDE.md`, `AGENTS.md`, ...) are only ever touched inside a delimited managed region, never overwritten wholesale.
- `shaic push` runs a secret-scan tripwire over staged content before committing — this also covers MCP server definitions, as a backstop against a literal credential accidentally being pasted into an `env` value instead of a `{ secret = "..." }` reference.
- MCP credential *values* are never stored by shaic either: they live only in this machine's OS keychain (`shaic mcp secret set`), resolved into the real agent config file at sync time and nowhere else. The canonical store only ever holds `{ secret = "NAME" }` references. `command`/`args` for MCP servers *do* sync via git like any other content — trust your remote the same way you already trust it for skill/rule bodies.
- v1 targets Unix (macOS/Linux) only.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
