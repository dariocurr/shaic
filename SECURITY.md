# Security

## Supported versions

Security fixes land on the latest tagged release.

## What shaic stores

- The canonical store is a git repo. Treat its remote like any other git
  remote that holds source code: `command` / `args` for MCP servers sync as
  content.
- MCP *credential values* live in the OS secret store (`shaic mcp secret set` —
  macOS Keychain, Linux Secret Service, Windows Credential Manager). They are
  never committed. Stdio secrets *are* written into agent config files on
  this machine at `shaic sync` time.
- Git remotes with embedded userinfo (`https://user:token@…`) are rejected.

## Reporting a vulnerability

Open a [private GitHub security advisory](https://github.com/dariocurr/shaic/security/advisories/new)
on this repository. Do not file a public issue for a credential leak or a
path-write escape.

Please include:

- shaic version (`shaic --version`)
- OS (macOS / Linux / Windows)
- What you expected vs what happened
- A minimal reproduction that does **not** include real secrets

## What we will not do

shaic does not phone home. There is no crash reporter and no telemetry.

`shaic doctor`, `shaic self check`, and `shaic self update` optionally contact
GitHub Releases to compare or download binaries. Nothing else is sent.
