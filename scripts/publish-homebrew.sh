#!/bin/sh
# Copy packaging/homebrew/shaic.rb into dariocurr/homebrew-tap and push.
set -eu

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: $0 <version>   e.g. 0.1.0" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/update-checksums.sh" "$VERSION"

TAP="${HOMEBREW_TAP_DIR:-$(mktemp -d)}"
cleanup() {
  if [ -z "${HOMEBREW_TAP_DIR:-}" ]; then
    rm -rf "$TAP"
  fi
}
trap cleanup EXIT

if [ -z "${HOMEBREW_TAP_DIR:-}" ]; then
  git clone --depth 1 git@github.com:dariocurr/homebrew-tap.git "$TAP"
fi

mkdir -p "$TAP/Formula"
cp "$ROOT/packaging/homebrew/shaic.rb" "$TAP/Formula/shaic.rb"

git -C "$TAP" add Formula/shaic.rb
if git -C "$TAP" diff --cached --quiet; then
  echo "tap already has shaic $VERSION"
  exit 0
fi

git -C "$TAP" commit -m "shaic ${VERSION}"
git -C "$TAP" push origin HEAD
echo "published dariocurr/tap/shaic ${VERSION}"
