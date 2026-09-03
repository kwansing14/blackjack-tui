class Blackjack < Formula
  desc "Two-player blackjack over TCP, played in the terminal"
  homepage "https://github.com/kwansing14/blackjack-tui"
  url "https://github.com/kwansing14/blackjack-tui/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "ea2d2a24ce32c2de74e2d64b2493180d34cdcffdbe7c659fb6ed0e791be82667"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
