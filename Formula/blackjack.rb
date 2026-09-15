class Blackjack < Formula
  desc "Blackjack in the terminal for a dealer and up to nine players, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.4.0"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.4.0/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "572f3cefdcaf73e3876763a7cc66a2baf66d5cd37d7a5a6ec73a0cce5e745b1a"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.4.0/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "be49c14933bce0243fb88f192df4d4a704667cd5d55e27dd04a40bd572ad59d6"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
