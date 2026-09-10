class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.2.0"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.0/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "ded3c24c1ef68dfe6a9f62f7fec8a4457bdb2a33b6a394451bb117fa069aa542"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.0/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "6ccf050c8f550b67570ef4b6c00908ca48a3cecd5a4e8e69340c397df29a7fd0"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
