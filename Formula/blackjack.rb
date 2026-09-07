class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.1.2"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.2/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "77f43f0018d17735ff5effc4a3f57f70e9fe2a2a376ff2e12bc3f746868383e9"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.2/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "547691e882ee74f67fecd0648cc8b07d68ce4efc98c67761d0934154bc49c168"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
