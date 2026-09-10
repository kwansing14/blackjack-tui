class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.2.1"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.1/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "3bb90d02189cf95fd7729df74d5210c3824389301e4e6c7c6c42bc6b2b7a5d2a"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.1/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "ecacf8119c3cce6385850c1cf6b7a32b8ecc99a4e271065ab7f75f68b6660e71"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
