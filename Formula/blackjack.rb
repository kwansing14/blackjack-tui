class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.1.1"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.1/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "6c1590c672caa9ef0e30b3ac462636667e94bb965b6fddd35bcbe033cef2344f"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.1/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "bdbfad428dc30878b025d09409b9e7d3e6976ec25f38724f1bebf4d368487b75"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
