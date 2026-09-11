# blackjack

Blackjack in the terminal for a dealer and up to nine players. One person hosts
and is the dealer; everyone else joins with a 4-letter room code and plays their
own hand against the dealer. Works across any network: every client talks over
plain HTTPS to a tiny Cloudflare Worker (a Durable Object per room) that relays
messages. No port forwarding, no IPs, no WebSockets, so it also works behind
corporate proxies.

## How online play works

```text
Host CLI (dealer) ── HTTPS (long-poll) ──┐
Player 1 CLI ────── HTTPS (long-poll) ──┤
    ...                                  ├── Cloudflare Worker ── Room Durable Object
Player 9 CLI ────── HTTPS (long-poll) ──┘                         (for example, KQZP)
```

Everyone talks to the hosted relay at `blackjack-relay.kwansing.workers.dev`
using ordinary HTTPS requests. The host runs the game and posts the whole table
after every move; each player posts their own moves and long-polls for the
host's updates. The room code sends everyone to the same Durable Object. The
players' computers never connect directly to each other.

Players only need the `blackjack` program and an internet connection. They do
not need:

- a Cloudflare account
- the same Wi-Fi or local network
- each other's IP address
- port forwarding or firewall configuration for inbound traffic

## Setup from scratch

These steps happen on different computers. The host creates the room and up to
nine players join it.

### 1. Install `blackjack` on every computer

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

### 2. The host opens a room

On the host's computer, run:

```sh
blackjack host
```

The first time you host or join, the game asks `your name?` before anything
else. That name is what the other players see next to your seat and what the
scoreboard keeps; it is saved, so you are only asked once (see [Scores](#scores)).

The game prints a four-letter room code and the command for the players:

```text
room KQZP open, waiting for players (up to 9) ...
players run:  blackjack join KQZP
```

Send the four-letter code—or the complete join command—to the players. Keep this
terminal open. Closing it ends the room for everyone.

### 3. Players join the room

On each player's computer, replace `KQZP` with the code from the host:

```sh
blackjack join KQZP
```

Codes are case-insensitive, so `blackjack join kqzp` also works. Each player is
given the lowest free seat, Player 1 to Player 9, and sees:

```text
joined room KQZP as player 1, waiting for the dealer ...
```

The host sees `player 1 joined`, and everyone's table gains a line for the new
seat. A tenth player is refused with `room full`. Players can join while a round
is being played; they are shown as `joins next round` and are dealt in from the
next deal.

### 4. Start and play the round

Everyone types `r` and presses Enter. Cards are dealt once every seat is ready:
two to each player and two to the dealer (the host). Players see only the
dealer's first card until every player has finished. A round cannot start until
at least one player has joined.

The `>` marker shows whose turn it is. Players act in seat order, Player 1
first:

- type `h` and press Enter to hit
- type `s` and press Enter to stand

A hand on 21 stands automatically, and a hand that busts is over at once. When
the last player has finished, the dealer's second card is revealed and the host
plays the dealer's hand the same way against every hand still standing. If all
the players bust, the dealer does not play.

Each player is settled separately against the dealer: WIN with a higher total
or when the dealer busts, LOSE with a lower total, PUSH on a tie. The result
appears on each player's line; the dealer's line has no single result.

After the round, everyone types `r` and presses Enter to play again. A player
who types `q` and presses Enter leaves the table and the game carries on
without them; if it was their turn, the turn passes on, and if the others were
only waiting on them to ready up, the round starts. The host typing `q` ends the
room for everyone.

## Command reference

| Command | Purpose |
|---------|---------|
| `blackjack host` | Create a room, print its code, and play as the dealer |
| `blackjack join KQZP` | Join an existing room using its code and play a hand against the dealer |
| `blackjack scores` | Print the lifetime leaderboard kept by the relay |
| `blackjack name` | Show the name and profile file this computer plays under |
| `blackjack name Zed` | Change that name; your record on the scoreboard follows you |
| `blackjack --version` | Print the installed version, useful when checking everyone is on the same one |

All in-game controls require Enter:

| Key | Action |
|-----|--------|
| `h` | Hit |
| `s` | Stand |
| `r` | Ready for the first or next round |
| `q` | Quit |

## Scores

Every seat line ends with who is sitting there and how they have done at this
table, for example `bob 2W 0L 1P`: hands won, lost and pushed since they sat
down. The dealer's line counts every hand played against them, so it grows
faster than any one player's. A player who leaves and comes back starts again
from `0W 0L 0P`.

```text
  Dealer        : 9♥ 5♥  = 14  [not ready]  alice 5W 3L 1P
  Player 1 (you): 10♠ 7♠  = 17  WIN  [not ready]  bob 2W 0L 1P
```

The relay also keeps a lifetime scoreboard across every room it has hosted.
`blackjack scores` prints it, with your own row marked:

```text
  #  name           W    L    P  hands
  1  alice         12    8    2     22
  2  bob            8   12    2     22  (you)
```

A hand counts once for the player and once, the other way round, for the dealer.
Only hands that were actually played are counted; readying up and leaving
change nothing.

### Who you are

Your profile is `~/.config/blackjack/player.json` (or
`$XDG_CONFIG_HOME/blackjack/player.json`): the name you gave and a random id
made on your first run. The id is what the scoreboard keys on, so two players
who both call themselves `alice` stay separate, and renaming yourself with
`blackjack name Zed` keeps your record. The id never appears on screen or in the
leaderboard; whoever has the file plays as you, so treat it like a saved game.

- `BLACKJACK_NAME=alice blackjack join KQZP` plays under that name for this run
  and saves it, without asking. Scripts and tests use this.
- `BLACKJACK_PROFILE=/path/to/other.json` uses another profile file, for two
  people sharing one computer. Two seats playing from the *same* profile are not
  scored against each other.
- A seat that gave the relay no name (an empty profile, or a client that could
  not save one) plays normally but is never scored.

## Run directly from a source checkout

If you prefer not to install the command globally, everyone can clone the
repository:

```sh
git clone https://github.com/kwansing14/blackjack-tui.git
cd blackjack-tui
```

Then run the appropriate command from the repository directory on each
computer:

```sh
# Host
cargo run --release -- host

# Each player, on their own computer
cargo run --release -- join KQZP
```

The first run compiles the program and can take a minute. Both commands use the
hosted Cloudflare relay automatically.

Everyone at the table must run 0.3.0 or later: the table now carries up to nine
hands, and an older client cannot read it. To replace an older installed binary
with your checkout, run `cargo install --path . --force`.

## Troubleshooting

- **`no such room`**: Check the code and confirm the host is still running
  `blackjack host`.
- **`room full`**: The room already has nine players.
- **The round does not start**: Every seat, the dealer included, must type `r`
  and press Enter, and at least one player must have joined.
- **`command not found: blackjack`**: Open a new terminal after installation or
  add `$HOME/.cargo/bin` to `PATH`.
- **`cannot reach relay`**: Confirm everyone has internet access and that
  outbound HTTPS on port 443 is allowed. The client honours `HTTPS_PROXY` if your
  network needs an explicit proxy.
- **`bad reply from relay`**: The relay is older than the client. Versions from
  0.3.0 need a relay that numbers seats; if you host your own relay, redeploy it
  from `worker/`.
- **`unreadable game message (is everyone on the same version?)`**: The host and
  a player are running different versions. Everyone needs 0.3.0 or later.
- **`no name given; set BLACKJACK_NAME or run: blackjack name <NAME>`**: The game
  needed a name and could not ask for one (no terminal, or an empty answer). Run
  `blackjack name Zed` once, or set `BLACKJACK_NAME`.
- **`warning: could not save .../player.json`**: The profile directory is not
  writable. The game goes on, but under a new id every run, so its hands are not
  scored. Fix the permissions or point `BLACKJACK_PROFILE` somewhere writable.
- **`this relay keeps no scores` or `usage: POST /room/<CODE>/...` from
  `blackjack scores`**: The relay has no scoreboard database (or predates it).
  Play works either way; see [Host your own relay](#host-your-own-relay-optional)
  for adding one.
- **`invalid peer certificate: UnknownIssuer`**: Versions before 0.1.2 trusted
  only Mozilla's root certificates, so they failed behind corporate TLS-inspection
  proxies such as Zscaler. Upgrade to 0.1.2 or later, which uses the system trust
  store.
- **`usage: POST /room/<CODE>/...` or `426 Upgrade Required` when hosting or
  joining**: Versions before 0.2.0 used WebSockets, which corporate proxies such as
  Zscaler block, and which the relay no longer speaks. Upgrade to 0.2.0 or later,
  which uses plain HTTPS long-polling.
- **`host quit` / `host timed out`**: The room ends when the host quits or is
  unreachable for about 45 seconds. The host can run `blackjack host` again to
  create a new room. A player who quits or drops only frees their seat; the host
  sees `player N left` and the game goes on.

## Host your own relay (optional)

Normal players can skip this section. The binary uses the hosted relay in
`src/net.rs` (`DEFAULT_SERVER`) by default. To deploy a separate relay with your
own Cloudflare account:

```sh
cd worker
npm install
npx wrangler login
npx wrangler d1 create blackjack-scores   # the scoreboard; prints a database_id
# paste that id into the "d1_databases" entry in wrangler.jsonc
npm run migrate                           # creates the tables (wrangler d1 migrations apply --remote)
npx wrangler deploy                       # prints https://blackjack-relay.<you>.workers.dev
```

The scoreboard is optional: leave out `d1_databases` and the relay still runs
games, answers `blackjack scores` with `this relay keeps no scores`, and ignores
the results the host reports. To look at the raw data:

```sh
npx wrangler d1 execute blackjack-scores --remote --command "SELECT * FROM hands ORDER BY played DESC LIMIT 20"
```

Then either change `DEFAULT_SERVER` and rebuild, or point every client at it:

```sh
# Host
BLACKJACK_SERVER=https://blackjack-relay.<you>.workers.dev blackjack host

# Each player
BLACKJACK_SERVER=https://blackjack-relay.<you>.workers.dev blackjack join KQZP
```

Everyone must set `BLACKJACK_SERVER` to the same address.

For local relay testing, run `npm run migrate:local` once and then
`npx wrangler dev` inside `worker/`, and start every client with
`BLACKJACK_SERVER=http://localhost:8787`. The local scoreboard lives under
`worker/.wrangler/state`, which is not committed.

The relay is a dumb pipe with ten mailboxes: seat 0 is the host, seats 1 to 9
the players. The host runs the game and sends state snapshots, which the relay
copies to every player; players send actions, which go to the host. Each client
`POST`s messages to `/room/CODE/send` and long-polls `/room/CODE/poll`, which the
Worker holds for up to 20 seconds until something arrives. Every event names
the seat it came from, and the host also hears `joined` (with the newcomer's
name) and `left` as seats change hands. A player silent for 45 seconds is shown
out of their seat; a host silent that long ends the room for everyone. There are
no WebSockets, so it works through proxies that only pass plain HTTPS.

The relay never reads the game state. Each client tells it who is sitting down
when it opens or joins a room, and after each round the host `POST`s the settled
hands to `/room/CODE/result` as seat numbers and outcomes. The relay matches
seats to the ids it was given and writes one row per hand to D1; `POST /scores`
is a `GROUP BY` over those rows. A round reported twice (a retried request) is
counted once, and a failed write is logged, never answered, so a slow or absent
database cannot hold up play.

## Running the tests

```sh
cargo test
```

Everything runs offline in a couple of seconds:

- `src/game.rs`: the rules (hand values, dealing to every seat, turn order around
  the table, busts, naturals, pushes, players joining and leaving mid-round,
  out-of-turn actions, per-seat tallies, and the JSON round trip of the shared
  state).
- `src/player.rs`: the saved profile (first-run prompt, `BLACKJACK_NAME`,
  renaming in place, name rules).
- `src/net.rs`: the relay protocol against a stand-in relay on localhost (bearer
  tokens and seat numbers, who is joining, `poll?after=` acknowledgements, resent
  events, `joined` with a name and `left`, 5xx retries, 4xx/410 disconnects,
  ordered `send?id=` posts, result reports that never block play, `leave` on
  quit, the `/scores` answer).
- `src/main.rs`: what each seat sees on the table (hidden hole card, turn marker,
  ready flags, WIN/LOSE/PUSH/BUST per player, names and tallies, prompts) and the
  leaderboard layout.
- `tests/cli.rs`: the built binary end to end against the fake relay (usage and exit
  codes, relay refusals, a missing name, `blackjack name`, `blackjack scores`, the
  host announcing a room and greeting a named joiner, the joiner drawing the
  first snapshot).

`e2e.py` is the one check that needs the real relay code: in `worker/`, run
`npm run migrate:local` once and then `npx wrangler dev`; then `cargo build` and
`python3 e2e.py` from the repository root. It plays a round with three players,
has one leave, fills the table to nine, checks that a tenth is refused, and reads
the round back from the scoreboard.

## Publishing a release (maintainers)

1. Set the new version in `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit.
2. Run `npm run release`.

The script builds the two macOS binaries, tags and pushes, creates the GitHub
release with the tarballs attached, rewrites `Formula/blackjack.rb` with the new
URLs and checksums, and pushes that commit. Homebrew users then get the new
version with `brew update && brew upgrade blackjack`.

If a run dies partway (network, `gh` login), fix the cause and run
`npm run release` again. Steps that already happened are skipped.

The relay is deployed separately: after changing `worker/`, run `npx wrangler
deploy` from that directory. A release that changes the wire format (as 0.3.0
did) needs the relay deployed first.
