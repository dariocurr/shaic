class Shaic < Formula
  desc "Sync AI-agent skills, rules, commands, and MCP servers via git"
  homepage "https://github.com/dariocurr/shaic"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.3.0/shaic-aarch64-apple-darwin.tar.gz"
      sha256 "625ecb227677888dbaddb3953e06478352b36642766d9701bb09406a20efceab"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.3.0/shaic-x86_64-apple-darwin.tar.gz"
      sha256 "824943c1f03edd78e2804acfaf151f25f58f1fb09908c4be1b4caf0afa6a49eb"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/dariocurr/shaic/releases/download/v0.3.0/shaic-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0e32512e284cf91c4fe1d5fe08aa4ae9f3739afcae48f20b209d903c430943c1"
    end
    on_intel do
      url "https://github.com/dariocurr/shaic/releases/download/v0.3.0/shaic-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "f343c1ba930a9b56d3c6401d12d92adaa4848026180fefcfa790512849d02361"
    end
  end

  def install
    bin.install "shaic"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/shaic --version")
  end
end
