# Signing and notarization

Release binaries are checksummed and attested (`attest-build-provenance`).
They are **not** Apple-notarized or Authenticode-signed until the secrets
below exist on the GitHub repo.

## Required GitHub secrets (optional)

| Secret | Used for |
| --- | --- |
| `CARGO_REGISTRY_TOKEN` | `cargo publish` on tagged releases |
| `APPLE_DEVELOPER_ID_P12` | base64-encoded Developer ID Application `.p12` |
| `APPLE_P12_PASSWORD` | password for that `.p12` |
| `APPLE_ID` | Apple ID for `notarytool` |
| `APPLE_TEAM_ID` | 10-character team id |
| `APPLE_APP_SPECIFIC_PASSWORD` | app-specific password for notarization |
| `WINDOWS_PFX` | base64-encoded Authenticode `.pfx` |
| `WINDOWS_PFX_PASSWORD` | password for that `.pfx` |

Without these, the Release workflow still publishes unsigned GitHub Release
assets with SHA-256 checksums. `shaic self update` and the install scripts
verify those checksums.

## After the first release

```bash
./scripts/update-checksums.sh 0.1.0
```

Then commit the rewritten Homebrew formula and winget manifest.
