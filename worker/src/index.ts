import { DurableObject } from "cloudflare:workers";

// ponytail: plain HTTPS long-polling, no WebSockets. Corporate proxies such as Zscaler
// strip the WebSocket upgrade but pass ordinary POSTs. The host stays authoritative; the
// DO is a row of mailboxes with a bell on each: seat 0 is the host, seats 1-9 the players.
//
//   POST /room/CODE/host            body = {id, name}  -> {token, seat: 0}  201 | 409 taken
//   POST /room/CODE/join            body = {id, name}  -> {token, seat: N}  200 | 404 no room | 409 full
//                                   the body is who is sitting down, for the scoreboard; an empty
//                                   body is an anonymous seat, which plays but is never scored
//   POST /room/CODE/send?id=N       body = message  204   (id dedups a retried send)
//                                   a player's message goes to the host; the host's to every player
//   POST /room/CODE/poll?after=N    -> {events:[{seq,from,text,name?}]}, held up to HOLD_MS
//                                   the host also hears "joined" (with the newcomer's name) and
//                                   "left" from a seat
//   POST /room/CODE/result          body = {round, results:[{seat, outcome}]}  204   host only;
//                                   the settled hands of one round, written to the scoreboard
//   POST /room/CODE/leave           -> 204; the host leaving closes the room (everyone else
//                                   gets 410), a player leaving frees their seat
//   send/poll/result/leave carry `Authorization: Bearer <token>`.
//
//   POST /scores                    body = {id?}  -> {rows:[{name,wins,losses,pushes,hands,you}]}
//                                   the lifetime leaderboard from D1; `you` marks the caller's row

const HOLD_MS = 20_000; // how long an empty /poll waits before answering
const STALE_MS = 45_000; // a player silent this long is gone (clients re-poll every ~20s)
const MAX_MSG = 64 * 1024;
const SEATS = 10; // the host and nine players
const HOST = 0;
const OUTCOMES = ["Win", "Lose", "Push"] as const;
const TOP = 50; // leaderboard rows

type Env = { ROOM: DurableObjectNamespace<Room>; DB?: D1Database };
type Outcome = (typeof OUTCOMES)[number];
type Who = { id: string; name: string };
type Envelope = { seq: number; from: number; text: string; name?: string };
type Player = { token: string; lastSeen: number; inbox: Envelope[]; nextSeq: number; lastId: number; who: Who | null };
type RoomState = { code: string; opened: number; seats: (Player | null)[]; closed: string | null; lastResult: number };
type HandResult = { seat: number; outcome: Outcome };
type Report = { round: number; results: HandResult[] };

const NO_STORE = { "Cache-Control": "no-store" };
// ponytail: reason goes in a header too, so a proxy that swallows error bodies still shows it
const reply = (why: string, status: number) =>
  new Response(why, { status, headers: { ...NO_STORE, "X-Reason": why } });
const json = (v: unknown, status = 200) =>
  new Response(JSON.stringify(v), { status, headers: { ...NO_STORE, "Content-Type": "application/json" } });
const newPlayer = (now: number, who: Who | null): Player =>
  ({ token: crypto.randomUUID(), lastSeen: now, inbox: [], nextSeq: 1, lastId: 0, who });
const int = (s: string | null) => { const n = Number(s ?? 0); return Number.isFinite(n) ? n : 0; };
const parse = (text: string): any => { try { return JSON.parse(text); } catch { return null; } };

/** `{id, name}` from a host/join body, or null for an empty or unusable one. */
const parseWho = (text: string): Who | null => {
  const v = parse(text);
  const id = typeof v?.id === "string" && /^[A-Za-z0-9_-]{8,64}$/.test(v.id) ? v.id : null;
  const name = typeof v?.name === "string" ? v.name.replace(/\p{C}/gu, "").trim().slice(0, 32) : "";
  return id && name ? { id, name } : null;
};

/** A well-formed result report, or null. */
const parseReport = (text: string): Report | null => {
  const v = parse(text);
  if (!Number.isInteger(v?.round) || v.round <= 0 || !Array.isArray(v?.results) || v.results.length >= SEATS) return null;
  const ok = v.results.every((r: any) => Number.isInteger(r?.seat) && r.seat > HOST && r.seat < SEATS && OUTCOMES.includes(r?.outcome));
  return ok ? { round: v.round, results: v.results.map((r: any) => ({ seat: r.seat, outcome: r.outcome })) } : null;
};

export class Room extends DurableObject<Env> {
  private room: RoomState | null = null;
  private bells = new Map<number, () => void>(); // resolves that seat's pending /poll

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      const stored = await ctx.storage.get<RoomState>("room");
      // a room from before seats were numbered is just gone; one from before the scoreboard gets its defaults
      this.room = stored?.seats ? { code: "", opened: 0, lastResult: 0, ...stored } : null;
    });
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const [, , code, verb] = url.pathname.split("/");
    const now = Date.now();
    await this.expire(now);
    if (verb === "host") return this.open(now, code, parseWho(await req.text()));
    if (verb === "join") return this.join(now, parseWho(await req.text()));

    const room = this.room;
    if (!room) return reply("no such room (check the code, or the host quit)", 404);
    const token = req.headers.get("Authorization")?.replace(/^Bearer\s+/i, "");
    const seat = room.seats.findIndex((p) => token && p?.token === token);
    if (seat < 0) return reply("not in this room", 401);
    if (room.closed) return reply(room.closed, 410);
    const me = room.seats[seat]!;
    me.lastSeen = now;

    switch (verb) {
      case "send": {
        const text = await req.text();
        if (!text || text.length > MAX_MSG) return reply("bad message size", 413);
        const id = int(url.searchParams.get("id"));
        if (id > me.lastId) { // ponytail: a retried send after a lost response is a no-op
          me.lastId = id;
          for (const to of this.hears(room, seat)) this.deliver(room, to, seat, text);
        }
        await this.save();
        return new Response(null, { status: 204, headers: NO_STORE });
      }
      case "poll": {
        const after = int(url.searchParams.get("after"));
        me.inbox = me.inbox.filter((e) => e.seq > after); // `after` acks everything up to it
        if (me.inbox.length === 0) {
          // wake on a delivery, on the hold expiring, or the moment someone we listen to would go stale
          const stale = this.hears(room, seat).map((s) => room.seats[s]!.lastSeen + STALE_MS);
          const deadline = Math.min(now + HOLD_MS, ...stale);
          await this.wait(seat, Math.max(0, deadline - now));
          if (this.room !== room) return reply("room was reopened", 410);
          await this.expire(Date.now());
          if (room.closed) return reply(room.closed, 410);
        }
        await this.save();
        return json({ events: me.inbox });
      }
      case "result": {
        if (seat !== HOST) return reply("dealer only", 403);
        const report = parseReport(await req.text());
        if (!report) return reply("bad report", 400);
        if (report.round > room.lastResult) { // a retried report is a no-op
          room.lastResult = report.round;
          this.record(room, report, now);
        }
        await this.save();
        return new Response(null, { status: 204, headers: NO_STORE });
      }
      case "leave":
        if (seat === HOST) await this.close("host quit");
        else await this.vacate(seat);
        return new Response(null, { status: 204, headers: NO_STORE });
    }
    return reply("unknown action", 404);
  }

  private async open(now: number, code: string, who: Who | null): Promise<Response> {
    if (this.room && !this.room.closed) return reply("room code taken, host again", 409);
    for (const ring of this.bells.values()) ring(); // stragglers from the dead room see 410
    const seats: (Player | null)[] = Array(SEATS).fill(null);
    seats[HOST] = newPlayer(now, who);
    this.room = { code, opened: now, seats, closed: null, lastResult: 0 };
    await this.save();
    return json({ token: seats[HOST]!.token, seat: HOST }, 201);
  }

  private async join(now: number, who: Who | null): Promise<Response> {
    const room = this.room;
    if (!room || room.closed) return reply("no such room (check the code, or the host quit)", 404);
    const seat = room.seats.findIndex((p, i) => i !== HOST && p === null); // the lowest free chair
    if (seat < 0) return reply(`room full (${SEATS - 1} players)`, 409);
    room.seats[seat] = newPlayer(now, who);
    this.deliver(room, HOST, seat, "joined", who?.name);
    await this.save();
    return json({ token: room.seats[seat]!.token, seat });
  }

  /** Who hears `from`: players talk to the host, the host talks to every player. */
  private hears(room: RoomState, from: number): number[] {
    if (from !== HOST) return [HOST];
    return room.seats.flatMap((p, i) => (i !== HOST && p ? [i] : []));
  }

  private deliver(room: RoomState, to: number, from: number, text: string, name?: string) {
    const p = room.seats[to];
    if (!p) return;
    p.inbox.push({ seq: p.nextSeq++, from, text, ...(name ? { name } : {}) });
    this.bells.get(to)?.();
  }

  private wait(seat: number, ms: number): Promise<void> {
    return new Promise((resolve) => {
      this.bells.get(seat)?.(); // a newer poll from the same seat supersedes the old one
      let timer: ReturnType<typeof setTimeout>;
      const ring = () => {
        clearTimeout(timer);
        if (this.bells.get(seat) === ring) this.bells.delete(seat);
        resolve();
      };
      timer = setTimeout(ring, ms);
      this.bells.set(seat, ring);
    });
  }

  /**
   * Writes a round's hands to the scoreboard, if this relay has one. Anonymous seats and a
   * dealer playing against their own profile are left out. Best effort: the game has already
   * moved on, so a failure is logged, not answered.
   */
  private record(room: RoomState, report: Report, now: number) {
    const db = this.env.DB;
    const dealer = room.seats[HOST]?.who;
    if (!db || !dealer) return;
    const hands = report.results.flatMap((r) => {
      const player = room.seats[r.seat]?.who;
      return player && player.id !== dealer.id ? [{ ...r, player }] : [];
    });
    if (hands.length === 0) return;
    const upsert = db.prepare(
      "INSERT INTO players (id, name, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3) " +
        "ON CONFLICT(id) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen",
    );
    const insert = db.prepare(
      "INSERT OR IGNORE INTO hands (room, opened, round, seat, dealer, player, outcome, played) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    );
    const batch = [upsert.bind(dealer.id, dealer.name, now)];
    for (const h of hands) {
      batch.push(upsert.bind(h.player.id, h.player.name, now));
      batch.push(insert.bind(room.code, room.opened, report.round, h.seat, dealer.id, h.player.id, h.outcome, now));
    }
    this.ctx.waitUntil(db.batch(batch).catch((e) => console.error("scoreboard write failed", e)));
  }

  /** A player's chair is freed and the host is told. Their own next request gets 401. */
  private async vacate(seat: number) {
    const room = this.room;
    if (!room || room.closed || !room.seats[seat]) return;
    room.seats[seat] = null;
    this.bells.get(seat)?.();
    this.deliver(room, HOST, seat, "left");
    await this.save();
  }

  /** The host silent past STALE_MS ends the room; a player silent that long is shown out. */
  private async expire(now: number) {
    const r = this.room;
    if (!r || r.closed) return;
    const gone = (p: Player | null) => p !== null && now - p.lastSeen > STALE_MS;
    if (gone(r.seats[HOST])) return this.close("host timed out");
    for (let s = HOST + 1; s < SEATS; s++) if (gone(r.seats[s])) await this.vacate(s);
  }

  private async close(why: string) {
    if (!this.room || this.room.closed) return;
    this.room.closed = why;
    for (const ring of this.bells.values()) ring();
    await this.save();
  }

  private save() {
    return this.ctx.storage.put("room", this.room);
  }
}

// Every hand counts once for the player and once, the other way round, for the dealer.
const LEADERBOARD =
  "WITH seat AS (" +
  "  SELECT player AS id, outcome FROM hands" +
  "  UNION ALL" +
  "  SELECT dealer, CASE outcome WHEN 'Win' THEN 'Lose' WHEN 'Lose' THEN 'Win' ELSE 'Push' END FROM hands" +
  ") " +
  "SELECT p.name, SUM(outcome = 'Win') AS wins, SUM(outcome = 'Lose') AS losses, SUM(outcome = 'Push') AS pushes, " +
  "  COUNT(*) AS hands, COALESCE(p.id = ?1, 0) AS you " +
  "FROM seat JOIN players p ON p.id = seat.id " +
  `GROUP BY p.id ORDER BY wins DESC, losses ASC, p.name LIMIT ${TOP}`;

type LeaderboardRow = { name: string; wins: number; losses: number; pushes: number; hands: number; you: number };

/** The lifetime leaderboard. Ids never leave the relay; the caller's own row is marked instead. */
async function scores(req: Request, env: Env): Promise<Response> {
  if (!env.DB) return reply("this relay keeps no scores", 501);
  const v = parse(await req.text());
  const id = typeof v?.id === "string" ? v.id : "";
  try {
    const { results } = await env.DB.prepare(LEADERBOARD).bind(id).all<LeaderboardRow>();
    return json({ rows: results.map((r) => ({ ...r, you: r.you === 1 })) });
  } catch (e) {
    console.error("scoreboard read failed", e);
    return reply("scoreboard unavailable, try again", 503);
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname === "/scores") return req.method === "POST" ? scores(req, env) : reply("POST only", 405);
    const m = url.pathname.match(/^\/room\/([A-Z0-9]{4,8})\/(host|join|send|poll|result|leave)$/);
    if (!m) return reply("usage: POST /room/<CODE>/{host|join|send|poll|result|leave}, POST /scores", 404);
    if (req.method !== "POST") return reply("POST only", 405);
    return env.ROOM.getByName(m[1]).fetch(req);
  },
};
