# e2e check: in worker/, run `npm run migrate:local` once, then `npx wrangler dev`; then
# `cargo build && python3 e2e.py` from the repo root.
# A host and three players play a round, one player leaves, the table fills to nine, a tenth is
# refused, the host quitting ends it for everyone, and the round shows up on the scoreboard.
import subprocess, re, time, os, sys, tempfile
SERVER = os.environ.get("BLACKJACK_SERVER", "http://localhost:8787")
B = "target/debug/blackjack"
RUN = f"e2e{os.getpid() % 10000}"  # fresh names and profiles every run, so the scoreboard counts are exact
PROFILES = tempfile.mkdtemp(prefix="blackjack-e2e-")
def env_for(name): return dict(os.environ, BLACKJACK_SERVER=SERVER, BLACKJACK_NAME=f"{RUN}-{name}", BLACKJACK_PROFILE=os.path.join(PROFILES, f"{name}.json"))
def spawn(name, *a): return subprocess.Popen([B,*a], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env_for(name), text=True)
def finish(p, name):
    try: out = p.communicate(timeout=5)[0]
    except subprocess.TimeoutExpired: p.kill(); out = p.communicate()[0]; print(f"!! {name} DID NOT EXIT")
    return out
def cmd(p, s): p.stdin.write(s+"\n"); p.stdin.flush(); time.sleep(0.8)
def refused(name, *a): return subprocess.run([B,*a], capture_output=True, text=True, env=env_for(name), timeout=5)

host = spawn("d", "host")
line = host.stdout.readline(); code = re.search(r"room (\w{4})", line).group(1); print("HOST:", line.strip())
p1 = spawn("p1", "join", code.lower()); time.sleep(0.8)
p2 = spawn("p2", "join", code); time.sleep(0.8)
p3 = spawn("p3", "join", code); time.sleep(0.8)
nosuch = refused("p4", "join", "ZZZZ"); print("NO ROOM:", nosuch.returncode, nosuch.stderr.strip())

for p in (host, p1, p2, p3): cmd(p, "r")
cmd(p1, "s"); cmd(p2, "s"); cmd(p3, "s"); cmd(host, "s")  # players in seat order, then the host as dealer
cmd(p2, "q"); p2out = finish(p2, "P2")  # one player leaves; the table carries on
extras = [spawn(f"x{i}", "join", code) for i in range(7)]; time.sleep(2.5)  # seats 2 and 4..9
full = refused("x7", "join", code); print("10TH PLAYER:", full.returncode, full.stderr.strip())
cmd(host, "q")
hout = finish(host, "HOST"); p1out = finish(p1, "P1"); p3out = finish(p3, "P3")
xouts = [finish(x, f"EXTRA{i}") for i, x in enumerate(extras)]
time.sleep(1.0)  # the relay writes the scoreboard after answering the host
scores = subprocess.run([B, "scores"], capture_output=True, text=True, env=env_for("d"), timeout=10)
print("---- HOST ----"); print(hout[-1500:]); print("---- P1 tail ----"); print(p1out[-600:]); print("---- P2 tail ----"); print(p2out[-400:])
print("---- SCORES ----"); print(scores.stdout, scores.stderr)

assert nosuch.returncode != 0 and "no such room" in nosuch.stderr
assert full.returncode != 0 and "room full" in full.stderr
assert "Dealer (you)" in hout and "Player 1 (you)" in p1out and "Player 2 (you)" in p2out and "Player 3 (you)" in p3out
assert "Player 1 (you)" not in hout and "Dealer (you)" not in p1out
assert "as player 1," in p1out and "as player 2," in p2out and "as player 3," in p3out
for n in (1, 2, 3): assert f"player {n} ({RUN}-p{n}) joined" in hout, f"host never saw player {n} by name"
assert "player 2 left" in hout
assert hout.count("player 2 (") == 2, "the freed seat 2 is handed to the next joiner"
for n in range(4, 10): assert f"player {n} (" in hout, f"host never saw player {n}"
def results_per_table(out): return [len(re.findall(r"  (WIN|LOSE|PUSH)", t.split("\n>")[0])) for t in out.split("===== Round 1 =====")[1:]]
for out, who in ((hout, "host"), (p1out, "P1"), (p2out, "P2"), (p3out, "P3")):
    assert 3 in results_per_table(out), f"{who} never saw a table with all three results"
assert results_per_table(hout)[-1] == 2, "after player 2 left, the host's table shows the two players still seated"
assert "host quit, bye" in p1out and "host quit, bye" in p3out and all("host quit, bye" in x for x in xouts)
assert "host quit" not in p2out, "player 2 had already left"
assert host.returncode == 0 and p1.returncode == 0 and p2.returncode == 0 and p3.returncode == 0 and all(x.returncode == 0 for x in extras)

# the table's running score: every player one hand, the dealer three, on the last table the host drew
def tally(out, name):
    m = re.findall(rf"  {re.escape(name)} (\d+)W (\d+)L (\d+)P", out)
    assert m, f"no tally for {name} in the output"
    return tuple(int(x) for x in m[-1])
assert sum(tally(hout, f"{RUN}-d")) == 3, "the dealer played three hands"
for n in (1, 3): assert sum(tally(hout, f"{RUN}-p{n}")) == 1 and sum(tally(p1out, f"{RUN}-p{n}")) == 1
assert sum(tally(p2out, f"{RUN}-p2")) == 1

# the scoreboard: same people, same hands, the caller marked
assert scores.returncode == 0, f"blackjack scores failed: {scores.stderr} (did you run `npm run migrate:local` in worker/?)"
def row(name):
    m = re.search(rf"^\s*\d+\s+{re.escape(name)}\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)(\s+\(you\))?$", scores.stdout, re.M)
    assert m, f"no leaderboard row for {name}"
    return tuple(int(x) for x in m.groups()[:4]) + (m.group(5) is not None,)
d = row(f"{RUN}-d"); assert d[3] == 3 and d[4], f"dealer row {d}"
for n in (1, 2, 3):
    r = row(f"{RUN}-p{n}"); assert r[3] == 1 and not r[4], f"p{n} row {r}"
assert d[0] == sum(row(f"{RUN}-p{n}")[1] for n in (1, 2, 3)), "the dealer's wins are the players' losses"
assert f"{RUN}-x0" not in scores.stdout, "a player who never played a hand has no row"
print("PASS: three players played the dealer, one left, the table filled to nine, a tenth was refused, the host closed the room, and the scoreboard has the round")
