# Packaging

Distribution artifacts for shaic outside GitHub Releases.

## Homebrew (macOS / Linux)

Users install from [dariocurr/homebrew-tap](https://github.com/dariocurr/homebrew-tap):

```bash
brew install dariocurr/tap/shaic
```

The tap formula installs prebuilt GitHub Release binaries. After a
release, run:

```bash
./scripts/publish-homebrew.sh 0.1.0
```

## winget (Windows)

Update `winget/dariocurr.shaic.yaml` with the release version and installer URL,
then submit via [winget-pkgs](https://github.com/microsoft/winget-pkgs).

## Self-update

Installed binaries can run:

```bash
shaic self check
shaic self update
```

Downloads are verified against per-asset `.sha256` files from the release.

## Refreshing checksums after a release

```bash
./scripts/update-checksums.sh 0.1.0
```

Rewrites `homebrew/shaic.rb` and `winget/dariocurr.shaic.yaml` from the tagged
release assets. Commit the result before opening a Homebrew tap PR or winget PR.

## Signing

See [SIGNING.md](SIGNING.md). Unsigned GitHub Release assets are the default
until Apple/Windows signing secrets are set on the repo.
