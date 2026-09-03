# blackjack

Two-player blackjack over TCP, played in the terminal. One player hosts, the other joins.

## Install (Homebrew, no Rust needed)

```sh
brew tap kwansing14/tap https://github.com/kwansing14/blackjack-tui
brew trust kwansing14/tap
brew install blackjack
```

Homebrew builds it locally; Rust is pulled in as a build-only dependency.

With Rust already installed: `cargo install --git https://github.com/kwansing14/blackjack-tui`

## Run

Host (listens on port 7878 by default). It prints the exact join command for player 2:

```sh
blackjack host
blackjack host 9000      # custom port
```

Player 2, on another machine:

```sh
blackjack join <host-ip>:7878
```

Same machine for testing: use `127.0.0.1`. Over the internet the host needs to
port-forward 7878 (or use something like Tailscale) and share their public IP.

## Controls

| Key | Action |
|-----|--------|
| `h` | Hit |
| `s` | Stand |
| `r` | Ready for next round |
| `q` | Quit |

Type the key and press Enter.
