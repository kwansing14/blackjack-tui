import { DurableObject } from "cloudflare:workers";

// ponytail: plain HTTPS long-polling, no WebSockets. Corporate proxies such as Zscaler
// strip the WebSocket upgrade but pass ordinary POSTs. Host stays authoritative; the
// DO is a pair of mailboxes with a bell on each.
//
//   POST /room/CODE/host            -> {token}      201 | 409 taken
//   POST /room/CODE/join            -> {token}      200 | 404 no room | 409 full
//   POST /room/CODE/send?id=N       body = message  204   (id dedups a retried send)
//   POST /room/CODE/poll?after=N    -> {events:[{seq,text}]}, held up to HOLD_MS
//   POST /room/CODE/leave           -> 204, the other player's next poll gets 410
//   send/poll/leave carry `Authorization: Bearer <token>`.

const HOLD_MS = 20_000; // how long an empty /poll waits before answering
const STALE_MS = 45_000; // a player silent this long is gone (clients re-poll every ~20s)
const MAX_MSG = 64 * 1024;

type Seat = "host" | "join";
type Envelope = { seq: number; text: string };
type Player = { token: string; lastSeen: number; inbox: Envelope[]; nextSeq: number; lastId: number };
type RoomState = { host: Player; join: Player | null; closed: string | null };

const NO_STORE = { "Cache-Control": "no-store" };
// ponytail: reason goes in a header too, so a proxy that swallows error bodies still shows it
const reply = (why: string, status: number) =>
  new Response(why, { status, headers: { ...NO_STORE, "X-Reason": why } });
const json = (v: unknown, status = 200) =>
  new Response(JSON.stringify(v), { status, headers: { ...NO_STORE, "Content-Type": "application/json" } });
const newPlayer = (now: number): Player =>
  ({ token: crypto.randomUUID(), lastSeen: now, inbox: [], nextSeq: 1, lastId: 0 });
const other = (s: Seat): Seat => (s === "host" ? "join" : "host");
const int = (s: string | null) => { const n = Number(s ?? 0); return Number.isFinite(n) ? n : 0; };

export class Room extends DurableObject {
  private room: RoomState | null = null;
  private bells = new Map<Seat, () => void>(); // resolves that seat's pending /poll

  constructor(ctx: DurableObjectState, env: unknown) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      this.room = (await ctx.storage.get<RoomState>("room")) ?? null;
    });
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const verb = url.pathname.split("/").pop();
    const now = Date.now();
    await this.expire(now);
    if (verb === "host") return this.open(now);
    if (verb === "join") return this.join(now);

    const room = this.room;
    if (!room) return reply("no such room (check the code, or the host quit)", 404);
    const token = req.headers.get("Authorization")?.replace(/^Bearer\s+/i, "");
    const seat = (["host", "join"] as Seat[]).find((s) => token && room[s]?.token === token);
    if (!seat) return reply("not in this room", 401);
    if (room.closed) return reply(room.closed, 410);
    const me = room[seat]!;
    me.lastSeen = now;

    switch (verb) {
      case "send": {
        const text = await req.text();
        if (!text || text.length > MAX_MSG) return reply("bad message size", 413);
        const id = int(url.searchParams.get("id"));
        if (id > me.lastId) { // ponytail: a retried send after a lost response is a no-op
          me.lastId = id;
          this.deliver(room, other(seat), text);
        }
        await this.save();
        return new Response(null, { status: 204, headers: NO_STORE });
      }
      case "poll": {
        const after = int(url.searchParams.get("after"));
        me.inbox = me.inbox.filter((e) => e.seq > after); // `after` acks everything up to it
        if (me.inbox.length === 0) {
          // wake on a delivery, on the hold expiring, or the moment the peer would go stale
          const peer = room[other(seat)];
          const deadline = Math.min(now + HOLD_MS, peer ? peer.lastSeen + STALE_MS : Infinity);
          await this.wait(seat, Math.max(0, deadline - now));
          if (this.room !== room) return reply("room was reopened", 410);
          await this.expire(Date.now());
          if (room.closed) return reply(room.closed, 410);
        }
        await this.save();
        return json({ events: me.inbox });
      }
      case "leave":
        await this.close("other player quit");
        return new Response(null, { status: 204, headers: NO_STORE });
    }
    return reply("unknown action", 404);
  }

  private async open(now: number): Promise<Response> {
    if (this.room && !this.room.closed) return reply("room code taken, host again", 409);
    for (const ring of this.bells.values()) ring(); // stragglers from the dead room see 410
    this.room = { host: newPlayer(now), join: null, closed: null };
    await this.save();
    return json({ token: this.room.host.token }, 201);
  }

  private async join(now: number): Promise<Response> {
    const room = this.room;
    if (!room || room.closed) return reply("no such room (check the code, or the host quit)", 404);
    if (room.join) return reply("room full", 409);
    room.join = newPlayer(now);
    this.deliver(room, "host", "joined");
    await this.save();
    return json({ token: room.join.token });
  }

  private deliver(room: RoomState, to: Seat, text: string) {
    const p = room[to];
    if (!p) return;
    p.inbox.push({ seq: p.nextSeq++, text });
    this.bells.get(to)?.();
  }

  private wait(seat: Seat, ms: number): Promise<void> {
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

  /** Anyone silent past STALE_MS ends the room for both. */
  private async expire(now: number) {
    const r = this.room;
    if (!r || r.closed) return;
    const gone = (p: Player | null) => p !== null && now - p.lastSeen > STALE_MS;
    if (gone(r.host) || gone(r.join)) await this.close("other player timed out");
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

export default {
  async fetch(req: Request, env: { ROOM: DurableObjectNamespace<Room> }): Promise<Response> {
    const m = new URL(req.url).pathname.match(/^\/room\/([A-Z0-9]{4,8})\/(host|join|send|poll|leave)$/);
    if (!m) return reply("usage: POST /room/<CODE>/{host|join|send|poll|leave}", 404);
    if (req.method !== "POST") return reply("POST only", 405);
    return env.ROOM.getByName(m[1]).fetch(req);
  },
};
