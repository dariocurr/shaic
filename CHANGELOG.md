# Changelog

<!-- markdownlint-disable MD024 -->

## 0.2.0

### Added

- OpenCode agent adapter: skills/commands under `.opencode/` and
  `~/.config/opencode/`, rules via `AGENTS.md`, MCP merge of the `mcp` key in
  `opencode.json` (OpenCode's local/remote entry shape).

## 0.1.0

First public release.

### Added

- Git-backed store for AI-agent skills, rules, commands, and MCP servers.
- `shaic sync` — store → agents only. `shaic import` — agents → store.
- `shaic tui` dashboard.
- macOS, Linux, and Windows (x64 and ARM64).
- OS keychain for secrets (never committed).
- `shaic self check` / `shaic self update` for GitHub Release binaries.
- Homebrew: `brew install dariocurr/tap/shaic`.
- Install scripts for macOS/Linux and Windows.
- JSON output and stable exit codes for scripting.
- Secret scan on push/pull.

### Fixed

- Frontmatter parse accepts CRLF so Windows clones with `core.autocrlf` still
  list store items. New store init/clone forces `core.autocrlf=false`.

### Notes

- `shaic import` is lossy across agents with different on-disk shapes.
- Google Antigravity is experimental and read-only.
