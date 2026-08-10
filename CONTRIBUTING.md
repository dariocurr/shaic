# Contributing

## Setup

Requires Rust (edition 2024) and `git` on `PATH`.

```sh
git clone <this repo>
cd shaic
cargo build --workspace
cargo test --workspace
```

Install the pre-commit hooks (runs `cargo fmt --check` and `cargo clippy -D warnings` before every commit):

```sh
pip install pre-commit   # or: brew install pre-commit
pre-commit install
```

## Project layout

- `core/` (`shaic-core`) — all business logic: canonical store, agent adapters, materialize/write path, security. No I/O-free code depends on `cli`/`tui`.
- `cli/` (`shaic`) — thin `clap`-based command layer over `shaic-core`.
- `tui/` (`shaic-tui`) — thin `ratatui`-based interactive layer over `shaic-core`.

Module style: no `mod.rs` files anywhere — a module with submodules is `<name>.rs` plus a sibling `<name>/` directory (e.g. `adapters.rs` + `adapters/cursor.rs`).

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must be clean. CI enforces the same.

## Security-sensitive areas

If your change touches any of these, call it out explicitly in the PR description and add a test:

- `core/src/security/path_guard.rs` — the only path-traversal/symlink-escape boundary in the codebase
- `core/src/materialize/writer.rs` — the only code allowed to write into an agent-owned directory
- `core/src/store/git.rs` — remote URL validation and the git shell-out surface

## Adding a new agent

1. Add a variant to `AgentId` in `core/src/model.rs`.
2. Add `core/src/adapters/<name>.rs` implementing the `Agent` trait (see any existing adapter for the shape — most are 30-60 lines using the shared helpers in `adapters/common.rs`).
3. Register it in `core/src/adapters.rs::registry()`.
4. Add render/discover tests following the pattern in existing adapter modules.

Keep new agents honest about what's actually confirmed: if you're not sure of the real on-disk convention, ship it `experimental_read_only()` (see `google_antigravity.rs`) rather than guessing at a write path.

`mcp_target()` (default `None`) is separate and optional: only override it if the agent has a config file *dedicated solely* to MCP servers (never mixed with unrelated settings — see `claude_code.rs`/`cursor.rs` for the shape). If the agent's MCP config shares a file with other settings, leave it unsupported rather than risking a blind-merge into content shaic doesn't own.
