# Contributing

## Setup

Requires Rust 1.85+ (edition 2024) and `git` on `PATH`. CI runs on Linux,
macOS, and Windows.

```sh
git clone <this repo>
cd shaic
cargo build --workspace
cargo test --workspace
```

Install the pre-commit hooks (markdownlint, trailing-whitespace / YAML / TOML
checks, `cargo fmt --check`, and `cargo clippy -D warnings` — **not**
`cargo test`; run tests yourself before a PR):

```sh
pip install pre-commit   # or: brew install pre-commit
pre-commit install
```

## Project layout

- `core/` (`shaic-core`) — all business logic: canonical store, agent adapters,
  materialize/write path, security. No I/O-free code depends on `cli`/`tui`.
- `cli/` (`shaic`) — thin `clap`-based command layer over `shaic-core`.
- `tui/` (`shaic-tui`) — thin `ratatui`-based interactive layer over
  `shaic-core`.

Module style: no `mod.rs` files anywhere — a module with submodules is
`<name>.rs` plus a sibling `<name>/` directory (e.g. `adapters.rs` +
`adapters/cursor.rs`).

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must be clean. CI enforces the same. Pre-commit hooks do **not**
run the test suite.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | General failure |
| 2 | Usage / user input error |
| 3 | Git error (diverged history, uncommitted pull, network) |
| 4 | Secret scan blocked the operation |
| 5 | Config / store not ready |

Scripts can branch on these instead of parsing stderr.

## Security-sensitive areas

If your change touches any of these, call it out explicitly in the PR
description and add a test:

- `core/src/security/path_guard.rs` — the only path-traversal/symlink-escape
  boundary in the codebase
- `core/src/materialize/writer.rs` — the only code allowed to write into an
  agent-owned directory
- `core/src/store/git.rs` — remote URL validation and the git shell-out surface

## Adding a new agent

1. Add a variant to `AgentId` in `core/src/model.rs`.
2. Add `core/src/adapters/<name>.rs` implementing the `Agent` trait (see any
   existing adapter for the shape — most are 30-60 lines using the shared
   helpers in `adapters/common.rs`).
3. Register it in `core/src/adapters.rs`: add it to the `REGISTRY` list and to
   `by_id()`'s match (the match is exhaustive, so step 1 won't compile until you
   do).
4. Add render/discover tests following the pattern in existing adapter modules.

Keep new agents honest about what's actually confirmed: if you're not sure of
the real on-disk convention, ship it `experimental_read_only()` (see
`google_antigravity.rs`) rather than guessing at a write path.

`mcp_target()` (default `None`) is separate and optional. Override it when shaic
can safely merge MCP servers into the agent's on-disk config — either a
dedicated MCP JSON file (see `cursor.rs`), OpenCode's shared `opencode.json`
`mcp` object (`OpenCodeJson`), or a shared settings file where only a named key
/ TOML table prefix is rewritten (see Claude Code global `~/.claude.json` and
Codex `~/.codex/config.toml`). If you cannot preserve unrelated settings with
certainty, leave MCP unsupported.
