class Blackjack < Formula
  desc "Two-player blackjack in the terminal, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.2.2"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.2/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "cb1879a92438404bc29f27665fa77eb45fb5c7145e37b76031d375013f6043fb"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.2.2/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "3a5a34e93a4a89c1b9aab253399f3fcc8c3b120bab48168fbda6cb3151d3948d"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
