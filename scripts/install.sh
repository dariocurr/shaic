#!/bin/sh
# Install the latest shaic release binary for this machine.
# macOS and Linux only.
set -eu

REPO="dariocurr/shaic"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${PREFIX}/bin"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
Darwin)
  case "$arch" in
  arm64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *)
    echo "unsupported macOS arch: $arch" >&2
    exit 1
    ;;
  esac
  ;;
Linux)
  case "$arch" in
  x86_64) target="x86_64-unknown-linux-gnu" ;;
  aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
  *)
    echo "unsupported Linux arch: $arch" >&2
    exit 1
    ;;
  esac
  ;;
*)
  echo "shaic supports macOS and Linux only (got $os)" >&2
  exit 1
  ;;
esac

asset="shaic-${target}.tar.gz"
sums="SHA256SUMS"
base="https://github.com/${REPO}/releases/latest/download"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "downloading ${asset}…"
curl -fsSL "${base}/${asset}" -o "${tmpdir}/${asset}"

verify_checksum() {
  if curl -fsSL "${base}/${asset}.sha256" -o "${tmpdir}/${asset}.sha256" 2>/dev/null; then
    (
      cd "$tmpdir"
      if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "${asset}.sha256"
      else
        shasum -a 256 -c "${asset}.sha256"
      fi
    )
    return
  fi
  curl -fsSL "${base}/${sums}" -o "${tmpdir}/${sums}"
  (
    cd "$tmpdir"
    if command -v sha256sum >/dev/null 2>&1; then
      grep " ${asset}\$" SHA256SUMS | sha256sum -c -
    else
      grep " ${asset}\$" SHA256SUMS | shasum -a 256 -c -
    fi
  )
}

verify_checksum

(
  cd "$tmpdir"
  tar -xzf "$asset"
)

mkdir -p "$BIN_DIR"
install -m 0755 "${tmpdir}/shaic" "${BIN_DIR}/shaic"
echo "installed ${BIN_DIR}/shaic"
echo "put ${BIN_DIR} on PATH if it is not already"
"${BIN_DIR}/shaic" --version
