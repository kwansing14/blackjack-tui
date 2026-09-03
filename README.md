# blackjack

Two-player blackjack over TCP, played in the terminal. One player hosts, the other joins.

## Build

Requires a Rust toolchain (`rustup`).

```sh
cargo build --release
```

## Run

Host (listens on port 7878 by default):

```sh
cargo run -- host
cargo run -- host 9000      # custom port
```

Join from a second terminal or machine:

```sh
cargo run -- join 127.0.0.1:7878
```

## Controls

| Key | Action |
|-----|--------|
| `h` | Hit |
| `s` | Stand |
| `r` | Ready for next round |
| `q` | Quit |

Type the key and press Enter.
