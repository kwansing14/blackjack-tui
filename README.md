# blackjack

Two-player blackjack in the terminal. One player hosts, the other joins with a
4-letter room code. Works across any network: both clients connect out to a tiny
Cloudflare Worker (a Durable Object per room) that relays messages. No port
forwarding, no IPs.

## How online play works

```text
Player 1 CLI ── secure WebSocket ──┐
                                   ├── Cloudflare Worker ── Room Durable Object
Player 2 CLI ── secure WebSocket ──┘                         (for example, KQZP)
```

Both players connect to the hosted relay at
`blackjack-relay.kwansing.workers.dev`. The room code sends both connections to
the same Durable Object. The two computers never connect directly to each
other.

Players only need the `blackjack` program and an internet connection. They do
not need:

- a Cloudflare account
- the same Wi-Fi or local network
- each other's IP address
- port forwarding or firewall configuration for inbound traffic

## Two-player setup from scratch

These steps happen on two different computers. Player 1 creates the room and
Player 2 joins it.

### 1. Install `blackjack` on both computers

#### Install with Rust

Install [Rust](https://www.rust-lang.org/tools/install) first, then run this on
each computer:

```sh
cargo install --git https://github.com/kwansing14/blackjack-tui --locked
```

This installs the `blackjack` command. Confirm it is available:

```sh
blackjack
```

It should print the command usage. If the shell says `command not found`, open a
new terminal or add `$HOME/.cargo/bin` to `PATH`.

#### Install on macOS with Homebrew

```sh
brew tap kwansing14/tap https://github.com/kwansing14/blackjack-tui
brew trust kwansing14/tap
brew install blackjack
```

Installs a prebuilt macOS binary. No Rust or other dependencies.

### 2. Player 1 hosts a room

On Player 1's computer, run:

```sh
blackjack host
```

The game prints a four-letter room code and the command for the other player:

```text
room KQZP open, waiting for player 2 ...
player 2 runs:  blackjack join KQZP
```

Send the four-letter code—or the complete join command—to Player 2. Keep this
terminal open. Closing it ends the room session and disconnects Player 2.

### 3. Player 2 joins the room

On Player 2's computer, replace `KQZP` with the code from Player 1:

```sh
blackjack join KQZP
```

Codes are case-insensitive, so `blackjack join kqzp` also works. When the
connection succeeds, Player 1 sees:

```text
player 2 connected
```

Only one Player 2 can join a room.

### 4. Start and play the round

Both players type `r` and press Enter. Cards are dealt once both players are
ready.

The `>` marker shows whose turn it is. On your turn:

- type `h` and press Enter to hit
- type `s` and press Enter to stand

Player 1 takes the first turn, followed by Player 2. The dealer then plays
automatically, and both results are displayed.

After the round, both players can type `r` and press Enter to play again. Either
player can type `q` and press Enter to quit; this also disconnects the other
player.

## Command reference

| Command | Purpose |
|---------|---------|
| `blackjack host` | Create a room and print its code |
| `blackjack join KQZP` | Join an existing room using its code |

All in-game controls require Enter:

| Key | Action |
|-----|--------|
| `h` | Hit |
| `s` | Stand |
| `r` | Ready for the first or next round |
| `q` | Quit |

## Run directly from a source checkout

If you prefer not to install the command globally, both players can clone the
repository:

```sh
git clone https://github.com/kwansing14/blackjack-tui.git
cd blackjack-tui
```

Then run the appropriate command from the repository directory on each
computer:

```sh
# Player 1
cargo run --release -- host

# Player 2, on the other computer
cargo run --release -- join KQZP
```

The first run compiles the program and can take a minute. Both commands use the
hosted Cloudflare relay automatically.

## Troubleshooting

- **`no such room`**: Check the code and confirm Player 1 is still running
  `blackjack host`.
- **`room full`**: The room already has a second player. Player 1 should quit and
  create a new room.
- **The round does not start**: Both players must type `r` and press Enter.
- **`command not found: blackjack`**: Open a new terminal after installation or
  add `$HOME/.cargo/bin` to `PATH`.
- **Connection failure**: Confirm both players have internet access and that
  outbound secure WebSocket traffic on port 443 is allowed.
- **`invalid peer certificate: UnknownIssuer`**: Versions before 0.1.2 trusted
  only Mozilla's root certificates, so they failed behind corporate TLS-inspection
  proxies such as Zscaler. Upgrade to 0.1.2 or later, which uses the system trust
  store.
- **`relay said 426 Upgrade Required`**: Something between you and the relay is
  stripping the WebSocket upgrade. Corporate proxies commonly do this. Try another
  network, such as a phone hotspot.
- **A player disconnected**: The room ends when either player quits or loses
  their connection. Player 1 can run `blackjack host` again to create a new room.

## Host your own relay (optional)

Normal players can skip this section. The binary uses the hosted relay in
`src/net.rs` (`DEFAULT_SERVER`) by default. To deploy a separate relay with your
own Cloudflare account:

```sh
cd worker
npm install
npx wrangler login
npx wrangler deploy      # prints https://blackjack-relay.<you>.workers.dev
```

Then either change `DEFAULT_SERVER` and rebuild, or point both clients at it:

```sh
# Host
BLACKJACK_SERVER=wss://blackjack-relay.<you>.workers.dev blackjack host

# Player 2
BLACKJACK_SERVER=wss://blackjack-relay.<you>.workers.dev blackjack join KQZP
```

Both players must set `BLACKJACK_SERVER` to the same address.

For local relay testing, run `npx wrangler dev` inside `worker/`, then start both
clients with `BLACKJACK_SERVER=ws://localhost:8787`.

The relay is a dumb pipe: the host runs the game and sends state snapshots, the
joiner sends actions. The Worker just pairs two WebSockets by room code and
forwards frames, using WebSocket hibernation while connections are idle.

## Publishing a release (maintainers)

1. Set the new version in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit.
2. Run `./scripts/release.sh`.

The script builds the two macOS binaries, tags and pushes, creates the GitHub
release with the tarballs attached, rewrites `Formula/blackjack.rb` with the new
URLs and checksums, and pushes that commit. Homebrew users then get the new
version with `brew update && brew upgrade blackjack`.
