class Blackjack < Formula
  desc "Blackjack in the terminal for a dealer and up to nine players, over the internet"
  homepage "https://github.com/kwansing14/blackjack-tui"
  version "0.3.0"

  on_arm do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.3.0/blackjack-aarch64-apple-darwin.tar.gz"
    sha256 "d4b6ef8c4d9383c9fddcece64843e8ce6e7391a7a6b09b7f6dcaf0e741bd2c38"
  end
  on_intel do
    url "https://github.com/kwansing14/blackjack-tui/releases/download/v0.3.0/blackjack-x86_64-apple-darwin.tar.gz"
    sha256 "53ab6756979e4ba0336c6dce6ca3588ebb295ab60acd3f8e6b69f552b628bd3b"
  end

  def install
    bin.install "blackjack"
  end

  test do
    assert_match "usage", shell_output("#{bin}/blackjack 2>&1", 2)
  end
end
