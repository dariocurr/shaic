class Shaic < Formula
  desc "Sync AI-agent skills, rules, commands, and MCP servers via git"
  homepage "https://github.com/dariocurr/shaic"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.2.0/shaic-aarch64-apple-darwin.tar.gz"
      sha256 "e2606b9b6da91917679b2f176805c5aa14586e14380fd7bdb8e3174923490f51"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.2.0/shaic-x86_64-apple-darwin.tar.gz"
      sha256 "65f91a1a8f0897318348dedafc3b3c40dafbd1ac92b229ae49baad256cbe95bc"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.2.0/shaic-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "8c66ca2e0ea36c41bd2d66ff34d85d21d25102f40a5b69d30cd438ecc1a62c4a"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.2.0/shaic-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c1994a1b7da7651de7e146bf00c485885bdc8e676dfa6fae6b327935e0f9ae3f"
    end
  end

  def install
    bin.install "shaic"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/shaic --version")
  end
end
