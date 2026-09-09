use crate::game::{Action, GameState};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::{self, BufRead};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use ureq::tls::{RootCerts, TlsConfig};
use ureq::Agent;

/// Cloudflare Worker in ./worker. Override with BLACKJACK_SERVER=https://...
pub const DEFAULT_SERVER: &str = "https://blackjack-relay.kwansing.workers.dev";

// ponytail: plain HTTPS long-polling instead of WebSockets, because corporate proxies
// (Zscaler) strip the upgrade. The relay holds an empty /poll for up to 20s, so that
// request's timeout sits above it; everything else is a quick round trip.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
const QUICK_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_EVERY: Duration = Duration::from_secs(1);
/// Keep retrying a flaky relay this long before calling the game over. The relay writes
/// a player off after 45s of silence anyway, so there is no point outlasting that.
const GIVE_UP: Duration = Duration::from_secs(40);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Action(Action),
    State(GameState),
}

pub enum Event {
    Line(String),
    Net(Msg),
    Peer,                 // relay says player 2 joined (host only)
    Disconnected(String), // why, in words the player can read
}

#[derive(Deserialize)]
struct Seat {
    token: String,
}
#[derive(Deserialize)]
struct Inbox {
    events: Vec<Envelope>,
}
#[derive(Deserialize)]
struct Envelope {
    seq: u64,
    text: String,
}

#[derive(Clone)]
struct Relay {
    agent: Agent,
    room: String, // https://host/room/CODE
    token: String,
}

impl Relay {
    /// One POST to the room. Ok((status, body)) for any HTTP answer, Err for no answer.
    fn post(&self, path: &str, body: &str, timeout: Duration) -> Result<(u16, String), String> {
        let mut req = self.agent.post(format!("{}/{path}", self.room));
        if !self.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.token));
        }
        let mut resp = req
            .config()
            .timeout_global(Some(timeout))
            .build()
            .send(body)
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, text))
    }

    /// Retries no-answer and 5xx for up to GIVE_UP. A 4xx is the relay's final word,
    /// returned as Ok for the caller to read.
    fn post_retrying(&self, path: &str, body: &str, timeout: Duration) -> Result<(u16, String), String> {
        let start = Instant::now();
        loop {
            let err = match self.post(path, body, timeout) {
                Ok((status, text)) if status < 500 => return Ok((status, text)),
                Ok((status, text)) => reason(status, &text),
                Err(e) => e,
            };
            if start.elapsed() >= GIVE_UP {
                return Err(err);
            }
            thread::sleep(RETRY_EVERY);
        }
    }
}

fn reason(status: u16, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("relay said {status}")
    } else {
        body.to_owned()
    }
}

/// Outgoing side of the connection. Incoming events arrive on the channel given to `connect`.
pub struct Conn {
    out: Sender<Msg>,
    sender_gone: Receiver<()>,
}

impl Conn {
    /// Queues a message; a dead relay surfaces as Event::Disconnected, not here.
    pub fn send(&self, msg: Msg) {
        let _ = self.out.send(msg);
    }

    /// Tells the relay we left, so the other player hears now rather than in 45s.
    pub fn close(self) {
        let Conn { out, sender_gone } = self;
        drop(out); // sender thread drains, posts /leave, exits ...
        let _ = sender_gone.recv_timeout(QUICK_TIMEOUT); // ... and that exit is what we wait for
    }
}

/// Opens (host) or joins the room and spawns the poll and send threads. Errors here are
/// the relay's own words (room full, no such room, ...) or a transport failure.
pub fn connect(code: &str, role: &str, events: Sender<Event>) -> Result<Conn, Box<dyn Error>> {
    let server = std::env::var("BLACKJACK_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into());
    // older setups said wss://; same relay, plain https now
    let server = server.replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
    // system trust store, so TLS-inspecting proxies with their own root work
    let tls = TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build();
    let agent = Agent::new_with_config(
        Agent::config_builder().http_status_as_error(false).tls_config(tls).build(),
    );
    let mut relay = Relay {
        agent,
        room: format!("{}/room/{code}", server.trim_end_matches('/')),
        token: String::new(),
    };
    let (status, body) = relay
        .post(role, "", QUICK_TIMEOUT)
        .map_err(|e| format!("cannot reach relay: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(reason(status, &body).into());
    }
    relay.token = serde_json::from_str::<Seat>(&body).map_err(|_| "bad reply from relay")?.token;

    let poller = relay.clone();
    let poll_events = events.clone();
    thread::spawn(move || poll_loop(poller, poll_events));

    let (out, out_rx) = mpsc::channel();
    let (gone_tx, sender_gone) = mpsc::channel();
    thread::spawn(move || {
        send_loop(relay, out_rx, events);
        drop(gone_tx);
    });
    Ok(Conn { out, sender_gone })
}

fn disconnect(events: &Sender<Event>, why: String) {
    let _ = events.send(Event::Disconnected(why));
}

/// Long-polls the relay forever; `after` acks what we have seen so nothing is lost or repeated.
fn poll_loop(relay: Relay, events: Sender<Event>) {
    let mut after = 0u64;
    loop {
        let (status, body) = match relay.post_retrying(&format!("poll?after={after}"), "", POLL_TIMEOUT) {
            Ok(r) => r,
            Err(e) => return disconnect(&events, format!("lost the relay ({e})")),
        };
        if status != 200 {
            return disconnect(&events, reason(status, &body)); // 410 peer left, 404 relay forgot us
        }
        let Ok(inbox) = serde_json::from_str::<Inbox>(&body) else {
            return disconnect(&events, "bad reply from relay".into());
        };
        for e in inbox.events {
            if e.seq <= after {
                continue; // relay resends until acked
            }
            after = e.seq;
            let event = if e.text == "joined" {
                Event::Peer
            } else {
                match serde_json::from_str::<Msg>(&e.text) {
                    Ok(m) => Event::Net(m),
                    // ponytail: malformed message = drop the peer
                    Err(_) => return disconnect(&events, "other player sent garbage".into()),
                }
            };
            if events.send(event).is_err() {
                return; // main is gone
            }
        }
    }
}

/// Posts queued messages in order. Each carries a serial so a retried send is not applied twice.
fn send_loop(relay: Relay, out: Receiver<Msg>, events: Sender<Event>) {
    let mut id = 0u64;
    while let Ok(m) = out.recv() {
        let Ok(text) = serde_json::to_string(&m) else { continue };
        id += 1;
        match relay.post_retrying(&format!("send?id={id}"), &text, QUICK_TIMEOUT) {
            Ok((204, _)) => {}
            Ok((status, body)) => return disconnect(&events, reason(status, &body)),
            Err(e) => return disconnect(&events, format!("lost the relay ({e})")),
        }
    }
    // channel closed = we are quitting; tell the relay so the other side hears now
    let _ = relay.post("leave", "", QUICK_TIMEOUT);
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
