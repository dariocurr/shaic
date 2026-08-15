#!/bin/sh
# Refresh packaging/homebrew/shaic.rb and packaging/winget/dariocurr.shaic.yaml
# checksums from a tagged GitHub Release.
set -eu

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: $0 <version>   e.g. 0.1.0" >&2
  exit 1
fi

TAG="v${VERSION}"
BASE="https://github.com/dariocurr/shaic/releases/download/${TAG}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

fetch_sha() {
  asset="$1"
  curl -fsSL "${BASE}/${asset}.sha256" | awk '{print $1}'
}

sha_linux_x64="$(fetch_sha shaic-x86_64-unknown-linux-gnu.tar.gz)"
sha_linux_arm="$(fetch_sha shaic-aarch64-unknown-linux-gnu.tar.gz)"
sha_mac_x64="$(fetch_sha shaic-x86_64-apple-darwin.tar.gz)"
sha_mac_arm="$(fetch_sha shaic-aarch64-apple-darwin.tar.gz)"
sha_win="$(fetch_sha shaic-x86_64-pc-windows-msvc.zip)"
sha_win_arm="$(fetch_sha shaic-aarch64-pc-windows-msvc.zip)"

cat >"${ROOT}/packaging/homebrew/shaic.rb" <<EOF
class Shaic < Formula
  desc "Sync AI-agent skills, rules, commands, and MCP servers via git"
  homepage "https://github.com/dariocurr/shaic"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "${BASE}/shaic-aarch64-apple-darwin.tar.gz"
      sha256 "${sha_mac_arm}"
    end
    on_intel do
      url "${BASE}/shaic-x86_64-apple-darwin.tar.gz"
      sha256 "${sha_mac_x64}"
    end
  end

  on_linux do
    on_arm do
      url "${BASE}/shaic-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_arm}"
    end
    on_intel do
      url "${BASE}/shaic-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${sha_linux_x64}"
    end
  end

  def install
    bin.install "shaic"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/shaic --version")
  end
end
EOF

cat >"${ROOT}/packaging/winget/dariocurr.shaic.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.singleton.1.5.0.schema.json
PackageIdentifier: dariocurr.shaic
PackageVersion: ${VERSION}
PackageLocale: en-US
Publisher: dariocurr
PackageName: shaic
License: MIT
ShortDescription: Sync AI-agent skills, rules, commands, and MCP servers via git
Moniker: shaic
Tags:
  - ai
  - git
  - mcp
Installers:
  - Architecture: x64
    InstallerType: zip
    NestedInstallerType: portable
    NestedInstallerFiles:
      - RelativeFilePath: shaic.exe
        PortableCommandAlias: shaic
    InstallerUrl: ${BASE}/shaic-x86_64-pc-windows-msvc.zip
    InstallerSha256: ${sha_win}
  - Architecture: arm64
    InstallerType: zip
    NestedInstallerType: portable
    NestedInstallerFiles:
      - RelativeFilePath: shaic.exe
        PortableCommandAlias: shaic
    InstallerUrl: ${BASE}/shaic-aarch64-pc-windows-msvc.zip
    InstallerSha256: ${sha_win_arm}
ManifestType: singleton
ManifestVersion: 1.5.0
EOF

echo "updated packaging checksums for ${TAG}"
