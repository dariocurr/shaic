#!/bin/sh
# Run the same gates as CI before tagging a release.
set -eu
cd "$(dirname "$0")/.."
echo "==> cargo fmt --check"
cargo fmt --all -- --check
echo "==> cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings
echo "==> cargo test"
cargo test --workspace
echo "==> release checks passed"
