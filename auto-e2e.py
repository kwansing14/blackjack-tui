# e2e check for `host --auto`, the house dealer: same setup as e2e.py -- in worker/, run
# `npm run migrate:local` once, then `npx wrangler dev`; then `cargo build && python3 auto-e2e.py`.
# A host with no stdin at all, as systemd runs it, readies itself, deals two rounds to two
# players who only ever say "r" and "s", draws its own hand to 17, and settles. Without --auto
# this table seats players and sits in WaitingForReady forever.
import subprocess, re, time, os, tempfile
SERVER = os.environ.get("BLACKJACK_SERVER", "http://localhost:8787")
B = "target/debug/blackjack"
RUN = f"auto{os.getpid() % 10000}"  # fresh names and profiles every run, so the scoreboard counts are exact
ROOM = f"HS{os.getpid() % 10000:04d}"  # a pinned code, which is the other half of what the house needs
PROFILES = tempfile.mkdtemp(prefix="blackjack-auto-e2e-")
def env_for(name): return dict(os.environ, BLACKJACK_SERVER=SERVER, BLACKJACK_NAME=f"{RUN}-{name}", BLACKJACK_PROFILE=os.path.join(PROFILES, f"{name}.json"))
def spawn(name, *a, stdin=subprocess.PIPE): return subprocess.Popen([B,*a], stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env_for(name), text=True)
def finish(p, name):
    try: out = p.communicate(timeout=6)[0]
    except subprocess.TimeoutExpired: p.kill(); out = p.communicate()[0]; print(f"!! {name} DID NOT EXIT")
    return out
def cmd(p, s): p.stdin.write(s+"\n"); p.stdin.flush(); time.sleep(0.8)

dealer = spawn("d", "host", "--auto", ROOM, stdin=subprocess.DEVNULL)  # StandardInput=null, as the unit has it
time.sleep(1.0)
p1 = spawn("p1", "join", ROOM); time.sleep(0.8)
p2 = spawn("p2", "join", ROOM); time.sleep(0.8)
for _ in range(2):  # twice, because the house has to ready itself again after settling
    cmd(p1, "r"); cmd(p2, "r")      # nobody readies the dealer
    cmd(p1, "s"); cmd(p2, "s")      # and nobody stands for it either
    time.sleep(1.5)
cmd(p1, "q"); cmd(p2, "q")
p1out, p2out = finish(p1, "P1"), finish(p2, "P2")
dealer.terminate(); dout = finish(dealer, "DEALER")
time.sleep(1.0)  # the relay writes the scoreboard after answering the host
scores = subprocess.run([B, "scores"], capture_output=True, text=True, env=env_for("d"), timeout=10)
print("---- DEALER (tail) ----"); print(dout[-1200:]); print("---- P1 (tail) ----"); print(p1out[-700:])

assert f"room {ROOM} open" in dout, "the house opened the code it was given, not four random letters"
assert "[ready]" in dout, "the house readied itself"
for out, who in ((dout, "dealer"), (p1out, "P1"), (p2out, "P2")):
    assert "===== Round 2 =====" in out, f"{who} never saw a second round: nothing deals without the house"
    assert re.search(r"  (WIN|LOSE|PUSH)", out), f"{who} never saw a settled hand"
assert re.search(r"Dealer \(you\)\s+:.*= (1[789]|2\d)", dout), f"the house never drew to 17:\n{dout[-500:]}"
def tally(out, name):
    m = re.findall(rf"  {re.escape(name)} (\d+)W (\d+)L (\d+)P", out)
    assert m, f"no tally for {name}"
    return tuple(int(x) for x in m[-1])
assert sum(tally(dout, f"{RUN}-d")) == 4, f"two rounds against two players is four hands for the house: {tally(dout, f'{RUN}-d')}"
for n in ("p1", "p2"): assert sum(tally(dout, f"{RUN}-{n}")) == 2, f"{n} played two hands"
assert scores.returncode == 0, f"blackjack scores failed: {scores.stderr} (did you run `npm run migrate:local` in worker/?)"
def row(name):
    m = re.search(rf"^\s*\d+\s+{re.escape(name)}\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)", scores.stdout, re.M)
    assert m, f"no leaderboard row for {name}"
    return tuple(int(x) for x in m.groups())
for n in ("p1", "p2"): assert row(f"{RUN}-{n}")[3] == 2, f"{n} should have two hands on the board"
# The house gets a row too. Not because the client reports one -- `state.players()` never
# includes seat 0 -- but because the relay writes dealer.id on every hand and its LEADERBOARD
# query UNIONs those back in, inverted (worker/src/index.ts). A permanent house dealer
# therefore accumulates every hand ever played on the box. See ssh-blackjack/docs/open-questions.md.
assert row(f"{RUN}-d")[3] == 4, "the relay scores the house, one row per player-hand"
print(f"PASS: `host --auto {ROOM}` dealt two rounds with stdin closed, drew its own hand to 17, and settled both")
