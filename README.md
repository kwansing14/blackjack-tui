# blackjack

Two-player blackjack in the terminal. One player hosts, the other joins with a
4-letter room code. Works across any network: both clients connect out to a tiny
Cloudflare Worker (a Durable Object per room) that relays messages. No port
forwarding, no IPs.

## Install (Homebrew, no Rust needed)

```sh
brew tap kwansing14/tap https://github.com/kwansing14/blackjack-tui
brew trust kwansing14/tap
brew install blackjack
```

Installs a prebuilt macOS binary. No Rust or other dependencies.

With Rust already installed: `cargo install --git https://github.com/kwansing14/blackjack-tui`

## Run

Host:

```sh
blackjack host
# room KQZP open, waiting for player 2 ...
# player 2 runs:  blackjack join KQZP
```

Player 2, anywhere:

```sh
blackjack join KQZP
```

## Controls

| Key | Action |
|-----|--------|
| `h` | Hit |
| `s` | Stand |
| `r` | Ready for next round |
| `q` | Quit |

Type the key and press Enter.

## Relay (Cloudflare Worker)

The binary defaults to the relay in `src/net.rs` (`DEFAULT_SERVER`). To run your
own, deploy `worker/` with a free Cloudflare account:

```sh
cd worker
npm install
npx wrangler login
npx wrangler deploy      # prints https://blackjack-relay.<you>.workers.dev
```

Then either change `DEFAULT_SERVER` and rebuild, or point clients at it:

```sh
BLACKJACK_SERVER=wss://blackjack-relay.<you>.workers.dev blackjack host
```

Local testing: `npx wrangler dev` in `worker/`, then `BLACKJACK_SERVER=ws://localhost:8787`
for both clients.

The relay is a dumb pipe: the host runs the game and sends state snapshots, the
joiner sends actions. The Worker just pairs two WebSockets by room code and
forwards frames. Rooms cost nothing while idle (WebSocket hibernation).
