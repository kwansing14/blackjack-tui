-- The leaderboard is now a week at a time, so it filters `hands` on a `played` range.
-- 0001 indexed only the two id columns, which left that filter scanning the table.
CREATE INDEX hands_played ON hands(played);
