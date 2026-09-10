-- The lifetime scoreboard. One row per settled hand; the leaderboard is a GROUP BY over it,
-- counting each hand once for the player and once, flipped, for the dealer.
CREATE TABLE players (
  id TEXT PRIMARY KEY,          -- the client's hidden profile id
  name TEXT NOT NULL,           -- whatever they last called themselves
  first_seen INTEGER NOT NULL,  -- ms since the epoch
  last_seen INTEGER NOT NULL
);

CREATE TABLE hands (
  room TEXT NOT NULL,           -- room code
  opened INTEGER NOT NULL,      -- when the host opened it; codes are reused, this makes the room unique
  round INTEGER NOT NULL,
  seat INTEGER NOT NULL,
  dealer TEXT NOT NULL,         -- players.id
  player TEXT NOT NULL,         -- players.id
  outcome TEXT NOT NULL CHECK (outcome IN ('Win', 'Lose', 'Push')),  -- the player's side
  played INTEGER NOT NULL,
  PRIMARY KEY (room, opened, round, seat)
);
CREATE INDEX hands_dealer ON hands(dealer);
CREATE INDEX hands_player ON hands(player);
