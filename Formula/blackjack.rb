class Blackjack < Formula
  desc "Two-player blackjack over TCP, played in the terminal"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.1.0"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.0/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "bd6c1bb9d61aa2f2ca5be0f8252058cd4b0dd55507f99b63450d0215faa3c390"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.1.0/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "2e2e372283267ac42614f9533f75cb3aa349719ff262e2ea7d175378c35e72c9"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
