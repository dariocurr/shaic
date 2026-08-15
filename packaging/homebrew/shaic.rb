class Shaic < Formula
  desc "Sync AI-agent skills, rules, commands, and MCP servers via git"
  homepage "https://github.com/dariocurr/shaic"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.1.0/shaic-aarch64-apple-darwin.tar.gz"
      sha256 "9783198f3b2ec61e6aa657bc63e52c4c067b3e2d1cea6eac6bb5bb8e97ee7f83"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.1.0/shaic-x86_64-apple-darwin.tar.gz"
      sha256 "a5db9f1a88fa6b39f864fa6c0c43c392dbd5a8229411680a51a13e50d36f6c00"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.1.0/shaic-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "8e477f65608be80c6b3cd7fa4cb45be4027f6c4213bf14c0ad77506a7a3227a9"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.1.0/shaic-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "990786401fbb09e5a37b697c0337e2ee172da10316c7ae3a76866ebcfcd2e006"
    end
  end

  def install
    bin.install "shaic"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/shaic --version")
  end
end
