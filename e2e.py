# e2e check: run `npx wrangler dev` in worker/, then `python3 e2e.py` from repo root.
import subprocess, re, time, os, sys
env = dict(os.environ, BLACKJACK_SERVER=os.environ.get("BLACKJACK_SERVER","ws://localhost:8787"))
B = "target/debug/blackjack"
def spawn(*a): return subprocess.Popen([B,*a], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env, text=True)
def finish(p, name):
    try: out = p.communicate(timeout=5)[0]
    except subprocess.TimeoutExpired: p.kill(); out = p.communicate()[0]; print(f"!! {name} DID NOT EXIT")
    return out
host = spawn("host")
line = host.stdout.readline(); code = re.search(r"room (\w{4})", line).group(1); print("HOST:", line.strip())
join = spawn("join", code.lower()); time.sleep(1)
bad = subprocess.run([B,"join",code], capture_output=True, text=True, env=env, timeout=5); print("3RD JOINER:", bad.returncode, bad.stderr.strip())
nosuch = subprocess.run([B,"join","ZZZZ"], capture_output=True, text=True, env=env, timeout=5); print("NO ROOM:", nosuch.returncode, nosuch.stderr.strip())
def cmd(p, s): p.stdin.write(s+"\n"); p.stdin.flush(); time.sleep(0.6)
cmd(host,"r"); cmd(join,"r"); cmd(host,"s"); cmd(join,"s"); cmd(join,"q")
jout = finish(join,"JOIN"); hout = finish(host,"HOST")
print("---- HOST ----"); print(hout[-1000:]); print("---- JOIN tail ----"); print(jout[-500:])
