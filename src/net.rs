use crate::game::{Action, GameState};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::{self, BufRead};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

/// Cloudflare Worker in ./worker. Override with BLACKJACK_SERVER=wss://...
pub const DEFAULT_SERVER: &str = "wss://blackjack-relay.kwansing14.workers.dev";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Action(Action),
    State(GameState),
}

pub enum Event {
    Line(String),
    Net(Msg),
    Peer, // relay says player 2 joined (host only)
    Disconnected,
}

/// Connects to the relay and spawns the socket thread. Returns the outgoing queue;
/// incoming frames arrive on `events`. Dropping the returned Sender closes the socket.
pub fn connect(
    code: &str,
    role: &str,
    events: Sender<Event>,
) -> Result<Sender<Msg>, Box<dyn Error>> {
    let server = std::env::var("BLACKJACK_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into());
    let (mut ws, _) =
        tungstenite::connect(format!("{server}/room/{code}?role={role}")).map_err(|e| match e {
            tungstenite::Error::Http(r) => r
                .headers()
                .get("x-reason")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("relay said {}", r.status())),
            e => e.to_string(),
        })?;
    // ponytail: one thread owns the socket. A short read timeout lets it poll the
    // outgoing queue between reads, so no mutex and no starvation. Turn-based game;
    // up to 50ms extra latency is fine.
    let tcp = match ws.get_mut() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_mut(),
        _ => unreachable!(),
    };
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_millis(50)))?;

    let (out_tx, out_rx) = mpsc::channel::<Msg>();
    thread::spawn(move || {
        loop {
            // drain outgoing first; on channel drop, close cleanly and stop
            loop {
                match out_rx.try_recv() {
                    Ok(m) => {
                        let Ok(text) = serde_json::to_string(&m) else {
                            break;
                        };
                        if ws.send(Message::text(text)).is_err() {
                            let _ = events.send(Event::Disconnected);
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = ws.close(None);
                        return;
                    }
                }
            }
            let event = match ws.read() {
                Ok(Message::Text(t)) if t.as_str() == "joined" => Event::Peer,
                Ok(Message::Text(t)) => match serde_json::from_str::<Msg>(&t) {
                    Ok(m) => Event::Net(m),
                    Err(_) => Event::Disconnected, // ponytail: malformed frame = drop the peer
                },
                Ok(Message::Close(_)) => Event::Disconnected,
                Ok(_) => continue, // ping/pong handled by tungstenite
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                Err(_) => Event::Disconnected,
            };
            let done = matches!(event, Event::Disconnected);
            if events.send(event).is_err() || done {
                return;
            }
        }
    });
    Ok(out_tx)
}

/// Queues a message; a dead socket surfaces as Event::Disconnected, not here.
pub fn send(out: &Sender<Msg>, msg: Msg) {
    let _ = out.send(msg);
}

/// One event per line typed on stdin.
pub fn spawn_input(tx: Sender<Event>) {
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if tx.send(Event::Line(line)).is_err() {
                return;
            }
        }
    });
}
