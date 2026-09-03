# blackjack

Two-player blackjack over TCP, played in the terminal. One player hosts, the other joins.

## Install

```sh
brew install rust          # or https://rustup.rs on Linux/Windows
cargo install --git https://github.com/kwansing14/blackjack-tui
```

This puts a `blackjack` command in `~/.cargo/bin`. Re-run the second line to update.

Working from a clone instead: `cargo install --path .`

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
