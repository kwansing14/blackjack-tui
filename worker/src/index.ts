import { DurableObject } from "cloudflare:workers";

// ponytail: dumb relay. Host stays authoritative; the DO just pairs two sockets.
export class Room extends DurableObject {
  async fetch(req: Request): Promise<Response> {
    const role = new URL(req.url).searchParams.get("role");
    const sockets = this.ctx.getWebSockets();
    // ponytail: reason goes in a header too; some WS clients drop the body of a failed upgrade
    const reject = (why: string, status: number) => new Response(why, { status, headers: { "X-Reason": why } });
    if (role === "host" && sockets.length > 0) return reject("room code taken, host again", 409);
    if (role !== "host" && sockets.length !== 1) {
      return sockets.length ? reject("room full", 409) : reject("no such room (check the code, or the host quit)", 404);
    }
    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server);
    if (role !== "host") sockets[0].send("joined");
    return new Response(null, { status: 101, webSocket: client });
  }

  webSocketMessage(ws: WebSocket, msg: string | ArrayBuffer) {
    for (const other of this.ctx.getWebSockets()) if (other !== ws) other.send(msg);
  }

  webSocketClose(ws: WebSocket, code: number, reason: string) {
    this.dropOthers(ws);
    try { ws.close(code, reason); } catch {} // completes the closing handshake
  }
  webSocketError(ws: WebSocket) { this.dropOthers(ws); }

  private dropOthers(ws: WebSocket) {
    for (const s of this.ctx.getWebSockets()) {
      if (s === ws) continue;
      try { s.close(1000, "peer left"); } catch {}
    }
  }
}

export default {
  async fetch(req: Request, env: { ROOM: DurableObjectNamespace<Room> }): Promise<Response> {
    const m = new URL(req.url).pathname.match(/^\/room\/([A-Z0-9]{4,8})$/);
    if (!m) return new Response("usage: /room/<CODE>?role=host|join", { status: 404 });
    if (req.headers.get("Upgrade") !== "websocket") return new Response("expected websocket", { status: 426 });
    return env.ROOM.getByName(m[1]).fetch(req);
  },
};
