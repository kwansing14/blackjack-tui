use crate::game::{Action, GameState, Outcome, DEALER, MAX_PLAYERS};
use crate::player::Profile;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::{self, BufRead, BufReader, Read};
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
/// A result report gets one quick try. It is bookkeeping, not play: the relay dedups it, and
/// the next table snapshot must not wait behind it.
const REPORT_TIMEOUT: Duration = Duration::from_secs(3);
/// Keep retrying a flaky relay this long before calling the game over. The relay writes
/// a player off after 45s of silence anyway, so there is no point outlasting that.
const GIVE_UP: Duration = Duration::from_secs(40);

/// What travels through the relay: players send actions to the host, the host sends
/// the whole table back to every player. The table is boxed so an Action stays small.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Action(Action),
    State(Box<GameState>),
}

/// One player's settled hand, as the host reports it to the relay for the scoreboard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandResult {
    pub seat: usize,
    pub outcome: Outcome, // the player's side
}

/// Everything settled in one round, for `Conn::report`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub round: u32,
    pub results: Vec<HandResult>,
}

/// One line of the lifetime leaderboard, as the relay answers `/scores`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Row {
    pub name: String,
    pub wins: u32,
    pub losses: u32,
    pub pushes: u32,
    pub hands: u32,
    pub you: bool,
}

#[derive(Debug)]
pub enum Event {
    Line(String),
    Net(usize, Msg),              // a message and the seat it came from
    Joined(usize, Option<String>), // relay says a player took that seat, and their name if they gave one (host only)
    Left(usize),                  // ... or gave it up, by quitting or going quiet (host only)
    Disconnected(String),         // why, in words the player can read
}

/// What the send thread posts, in order: game messages to `send`, reports to `result`.
enum Outgoing {
    Msg(Msg),
    Report(Report),
}

/// The relay's answer to host/join: our bearer token and where we sit.
#[derive(Deserialize)]
struct Ticket {
    token: String,
    seat: usize,
}
#[derive(Deserialize)]
struct Inbox {
    events: Vec<Envelope>,
}
#[derive(Deserialize)]
struct Envelope {
    seq: u64,
    from: usize,
    text: String,
    #[serde(default)]
    name: Option<String>, // on "joined": what the newcomer calls themselves
}
#[derive(Deserialize)]
struct Scores {
    rows: Vec<Row>,
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
    out: Sender<Outgoing>,
    sender_gone: Receiver<()>,
}

impl Conn {
    /// Queues a message; a dead relay surfaces as Event::Disconnected, not here.
    pub fn send(&self, msg: Msg) {
        let _ = self.out.send(Outgoing::Msg(msg));
    }

    /// Host-only: queues a round's results for the scoreboard. Best effort; the game never
    /// hears whether it landed. Queued behind the snapshot it belongs to and ahead of `leave`.
    pub fn report(&self, report: Report) {
        let _ = self.out.send(Outgoing::Report(report));
    }

    /// Tells the relay we left, so the others hear now rather than in 45s.
    pub fn close(self) {
        let Conn { out, sender_gone } = self;
        drop(out); // sender thread drains, posts /leave, exits ...
        let _ = sender_gone.recv_timeout(QUICK_TIMEOUT); // ... and that exit is what we wait for
    }
}

/// HTTP client for every request. System trust store, so TLS-inspecting proxies with
/// their own root work; 4xx/5xx come back as answers to read, not as errors.
fn agent() -> Agent {
    let tls = TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build();
    Agent::new_with_config(Agent::config_builder().http_status_as_error(false).tls_config(tls).build())
}

/// `https://host`. Older setups said wss://; same relay, plain https now.
fn server_url(server: &str) -> String {
    let server = server.replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
    server.trim_end_matches('/').to_owned()
}

/// `https://host/room/CODE`
fn room_url(server: &str, code: &str) -> String {
    format!("{}/room/{code}", server_url(server))
}

fn server() -> String {
    std::env::var("BLACKJACK_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.into())
}

/// Opens (host) or joins the room as `who` and spawns the poll and send threads. Returns the
/// connection and our seat number: 0 for the host, 1 to 9 for a joiner. Errors here are
/// the relay's own words (room full, no such room, ...) or a transport failure.
pub fn connect(code: &str, role: &str, who: &Profile, events: Sender<Event>) -> Result<(Conn, usize), Box<dyn Error>> {
    connect_to(&server(), code, role, who, events)
}

/// `connect` against a given relay, so tests can point it at a stand-in on localhost.
fn connect_to(server: &str, code: &str, role: &str, who: &Profile, events: Sender<Event>) -> Result<(Conn, usize), Box<dyn Error>> {
    let mut relay = Relay {
        agent: agent(),
        room: room_url(server, code),
        token: String::new(),
    };
    let who = serde_json::to_string(who).map_err(|e| e.to_string())?;
    let (status, body) = relay
        .post(role, &who, QUICK_TIMEOUT)
        .map_err(|e| format!("cannot reach relay: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(reason(status, &body).into());
    }
    let ticket = serde_json::from_str::<Ticket>(&body).map_err(|_| "bad reply from relay")?;
    let seat_fits = if role == "host" { ticket.seat == DEALER } else { (1..=MAX_PLAYERS).contains(&ticket.seat) };
    if !seat_fits {
        return Err("bad reply from relay".into());
    }
    relay.token = ticket.token;

    let poller = relay.clone();
    let poll_events = events.clone();
    thread::spawn(move || poll_loop(poller, poll_events));

    let (out, out_rx) = mpsc::channel();
    let (gone_tx, sender_gone) = mpsc::channel();
    thread::spawn(move || {
        send_loop(relay, out_rx, events);
        drop(gone_tx);
    });
    Ok((Conn { out, sender_gone }, ticket.seat))
}

/// The lifetime leaderboard from the relay, with `who`'s own row marked. Errors are the
/// relay's words (an old relay answers with its usage line) or a transport failure.
pub fn scores(who: &Profile) -> Result<Vec<Row>, Box<dyn Error>> {
    scores_from(&server(), who)
}

fn scores_from(server: &str, who: &Profile) -> Result<Vec<Row>, Box<dyn Error>> {
    let relay = Relay { agent: agent(), room: server_url(server), token: String::new() };
    let body = serde_json::json!({ "id": who.id }).to_string();
    let (status, text) = relay.post("scores", &body, QUICK_TIMEOUT).map_err(|e| format!("cannot reach relay: {e}"))?;
    if status != 200 {
        return Err(reason(status, &text).into());
    }
    Ok(serde_json::from_str::<Scores>(&text).map_err(|_| "bad reply from relay")?.rows)
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
            return disconnect(&events, reason(status, &body)); // 410 host left, 404 relay forgot us
        }
        let Ok(inbox) = serde_json::from_str::<Inbox>(&body) else {
            return disconnect(&events, "bad reply from relay".into());
        };
        for e in inbox.events {
            if e.seq <= after {
                continue; // relay resends until acked
            }
            after = e.seq;
            let event = match e.text.as_str() {
                "joined" => Event::Joined(e.from, e.name.filter(|n| !n.is_empty())),
                "left" => Event::Left(e.from),
                text => match serde_json::from_str::<Msg>(text) {
                    Ok(m) => Event::Net(e.from, m),
                    // ponytail: a message we cannot read means a client we cannot play with
                    Err(_) => return disconnect(&events, "unreadable game message (is everyone on the same version?)".into()),
                },
            };
            if events.send(event).is_err() {
                return; // main is gone
            }
        }
    }
}

/// Posts queued messages in order. Each carries a serial so a retried send is not applied twice.
/// A report gets a single try and no verdict: a relay that keeps no scores answers 404, and
/// the game goes on regardless.
fn send_loop(relay: Relay, out: Receiver<Outgoing>, events: Sender<Event>) {
    let mut id = 0u64;
    while let Ok(m) = out.recv() {
        match m {
            Outgoing::Msg(m) => {
                let Ok(text) = serde_json::to_string(&m) else { continue };
                id += 1;
                match relay.post_retrying(&format!("send?id={id}"), &text, QUICK_TIMEOUT) {
                    Ok((204, _)) => {}
                    Ok((status, body)) => return disconnect(&events, reason(status, &body)),
                    Err(e) => return disconnect(&events, format!("lost the relay ({e})")),
                }
            }
            Outgoing::Report(r) => {
                let Ok(text) = serde_json::to_string(&r) else { continue };
                let _ = relay.post("result", &text, REPORT_TIMEOUT);
            }
        }
    }
    // channel closed = we are quitting; tell the relay so the others hear now
    let _ = relay.post("leave", "", QUICK_TIMEOUT);
}

/// One event per line typed on stdin.
pub fn spawn_input(tx: Sender<Event>) {
    spawn_lines(io::stdin(), tx);
}

/// One Event::Line per line of `input`; the thread ends at EOF or once main is gone.
fn spawn_lines<R: Read + Send + 'static>(input: R, tx: Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(input).lines() {
            let Ok(line) = line else { return };
            if tx.send(Event::Line(line)).is_err() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Card, Phase, Seat, Tally};
    use std::collections::VecDeque;
    use std::io::{Cursor, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::RecvTimeoutError;
    use std::sync::{Arc, Mutex};

    /// Long enough for a thread hand-off on a busy CI box, short enough to notice a hang.
    const WAIT: Duration = Duration::from_secs(5);
    /// How long a test waits to be sure an event is *not* coming.
    const QUIET: Duration = Duration::from_millis(300);

    // ---- pure pieces --------------------------------------------------------------

    #[test]
    fn reason_is_the_relays_body_or_else_its_status() {
        assert_eq!(reason(404, ""), "relay said 404");
        assert_eq!(reason(502, " \n"), "relay said 502");
        assert_eq!(reason(409, "room full\n"), "room full");
    }

    /// Whoever is running the tests.
    fn who() -> Profile {
        Profile { id: "0123456789abcdef0123456789abcdef".into(), name: "tester".into() }
    }

    #[test]
    fn room_urls_are_plain_http_under_room() {
        assert_eq!(room_url("https://relay.example", "KQZP"), "https://relay.example/room/KQZP");
        assert_eq!(room_url("https://relay.example/", "KQZP"), "https://relay.example/room/KQZP");
        assert_eq!(room_url("wss://relay.example", "KQZP"), "https://relay.example/room/KQZP");
        assert_eq!(room_url("ws://localhost:8787", "KQZP"), "http://localhost:8787/room/KQZP");
        assert_eq!(server_url("https://relay.example/"), "https://relay.example");
    }

    /// Host and joiners may run different versions; this JSON is the contract between them.
    #[test]
    fn the_wire_format_is_stable() {
        assert_eq!(msg(Msg::Action(Action::Hit)), r#"{"Action":"Hit"}"#);
        assert_eq!(msg(Msg::Action(Action::Stand)), r#"{"Action":"Stand"}"#);
        assert_eq!(msg(Msg::Action(Action::Ready)), r#"{"Action":"Ready"}"#);
        let mut g = GameState::new();
        g.seats[2] = Some(Seat { hand: vec![Card { rank: 1, suit: 2 }], ready: true, ..Default::default() });
        g.phase = Phase::PlayerTurn;
        g.turn = 2;
        g.round = 3;
        let json = msg(Msg::State(Box::new(g.clone())));
        assert_eq!(
            json,
            concat!(
                r#"{"State":{"seats":[{"hand":[],"ready":false,"result":null,"name":"","tally":{"wins":0,"losses":0,"pushes":0}},null,"#,
                r#"{"hand":[{"rank":1,"suit":2}],"ready":true,"result":null,"name":"","tally":{"wins":0,"losses":0,"pushes":0}},"#,
                r#"null,null,null,null,null,null,null],"phase":"PlayerTurn","turn":2,"round":3}}"#
            )
        );
        match serde_json::from_str::<Msg>(&json).unwrap() {
            Msg::State(back) => assert_eq!(*back, g),
            other => panic!("decoded as {other:?}"),
        }
        let seat = Seat { name: "bob".into(), tally: Tally { wins: 2, losses: 0, pushes: 1 }, ..Default::default() };
        assert_eq!(
            serde_json::to_string(&seat).unwrap(),
            r#"{"hand":[],"ready":false,"result":null,"name":"bob","tally":{"wins":2,"losses":0,"pushes":1}}"#
        );
    }

    /// The relay reads these two, so their shape is a contract as well.
    #[test]
    fn the_relay_facing_formats_are_stable() {
        assert_eq!(serde_json::to_string(&who()).unwrap(), r#"{"id":"0123456789abcdef0123456789abcdef","name":"tester"}"#);
        let report = Report { round: 3, results: vec![HandResult { seat: 1, outcome: Outcome::Win }, HandResult { seat: 4, outcome: Outcome::Push }] };
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            r#"{"round":3,"results":[{"seat":1,"outcome":"Win"},{"seat":4,"outcome":"Push"}]}"#
        );
        let row: Row = serde_json::from_str(r#"{"name":"alice","wins":12,"losses":8,"pushes":2,"hands":22,"you":true}"#).unwrap();
        assert_eq!(row, Row { name: "alice".into(), wins: 12, losses: 8, pushes: 2, hands: 22, you: true });
    }

    #[test]
    fn input_lines_become_events_until_eof() {
        let (tx, rx) = mpsc::channel();
        spawn_lines(Cursor::new("h\n\nquit\n"), tx);
        let line = |e: Event| match e {
            Event::Line(l) => l,
            other => panic!("expected a line, got {other:?}"),
        };
        assert_eq!(line(rx.recv_timeout(WAIT).unwrap()), "h");
        assert_eq!(line(rx.recv_timeout(WAIT).unwrap()), "");
        assert_eq!(line(rx.recv_timeout(WAIT).unwrap()), "quit");
        assert_eq!(rx.recv_timeout(WAIT).unwrap_err(), RecvTimeoutError::Disconnected, "the reader hangs up at EOF");
    }

    // ---- a stand-in relay on localhost --------------------------------------------------

    #[derive(Clone, Debug)]
    struct Req {
        path: String,
        auth: Option<String>,
        body: String,
    }

    type Reply = (u16, String);
    type Handler = Arc<dyn Fn(&Req) -> Reply + Send + Sync>;

    struct FakeRelay {
        port: u16,
        seen: Arc<Mutex<Vec<Req>>>,
    }

    impl FakeRelay {
        /// Answers every request with `handler`, logging each one first.
        fn start(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
            let handler: Handler = Arc::new(handler);
            let log = seen.clone();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (handler, log) = (handler.clone(), log.clone());
                    thread::spawn(move || serve(stream, handler, log));
                }
            });
            FakeRelay { port, seen }
        }

        /// The usual relay: `open` answers host/join, polls are answered from `polls` in
        /// order and then held briefly and returned empty like the real one, sends get `on_send`,
        /// a report is taken with a 204.
        fn scripted(open: Reply, polls: Vec<Reply>, on_send: Reply) -> Self {
            Self::scripted_with_results(open, polls, on_send, (204, String::new()))
        }

        fn scripted_with_results(open: Reply, polls: Vec<Reply>, on_send: Reply, on_result: Reply) -> Self {
            let polls = Mutex::new(VecDeque::from(polls));
            Self::start(move |req| match verb(&req.path) {
                "host" | "join" => open.clone(),
                "poll" => polls.lock().unwrap().pop_front().unwrap_or_else(|| {
                    thread::sleep(Duration::from_millis(200));
                    inbox(&[])
                }),
                "send" => on_send.clone(),
                "result" => on_result.clone(),
                "leave" => (204, String::new()),
                _ => (404, "unknown action".into()),
            })
        }

        fn server(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn relay(&self, token: &str) -> Relay {
            Relay { agent: agent(), room: room_url(&self.server(), "ABCD"), token: token.into() }
        }

        fn seen(&self) -> Vec<Req> {
            self.seen.lock().unwrap().clone()
        }

        /// Blocks until at least `n` requests have arrived.
        fn wait_for(&self, n: usize) -> Vec<Req> {
            self.wait_until(|seen| seen.len() >= n, &format!("{n} requests"))
        }

        /// Blocks until at least `n` requests for one verb have arrived; returns just those.
        fn wait_for_verb(&self, want: &str, n: usize) -> Vec<Req> {
            let only = |seen: Vec<Req>| -> Vec<Req> { seen.into_iter().filter(|r| verb(&r.path) == want).collect() };
            only(self.wait_until(|seen| only(seen.to_vec()).len() >= n, &format!("{n} {want} requests")))
        }

        fn wait_until(&self, done: impl Fn(&[Req]) -> bool, what: &str) -> Vec<Req> {
            let deadline = Instant::now() + WAIT;
            loop {
                let seen = self.seen();
                if done(&seen) {
                    return seen;
                }
                assert!(Instant::now() < deadline, "never saw {what}; got {seen:#?}");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// `/room/ABCD/poll?after=3` -> `poll`
    fn verb(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("")
    }

    fn paths(reqs: &[Req]) -> Vec<&str> {
        reqs.iter().map(|r| r.path.as_str()).collect()
    }

    fn ticket(status: u16, token: &str, seat: usize) -> Reply {
        (status, format!(r#"{{"token":"{token}","seat":{seat}}}"#))
    }

    fn open_ok() -> Reply {
        ticket(201, "tok", 0)
    }

    fn sent() -> Reply {
        (204, String::new())
    }

    /// A 200 poll answer carrying these (seq, from, text) envelopes.
    fn inbox(events: &[(u64, usize, String)]) -> Reply {
        let events: Vec<_> = events
            .iter()
            .map(|(seq, from, text)| serde_json::json!({ "seq": seq, "from": from, "text": text }))
            .collect();
        (200, serde_json::json!({ "events": events }).to_string())
    }

    /// A 200 poll answer with one "joined" envelope that names the newcomer.
    fn joined_as(seq: u64, from: usize, name: &str) -> Reply {
        (200, serde_json::json!({ "events": [{ "seq": seq, "from": from, "text": "joined", "name": name }] }).to_string())
    }

    fn msg(m: Msg) -> String {
        serde_json::to_string(&m).unwrap()
    }

    fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => false,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                true
            }
        }
    }

    /// Just enough HTTP/1.1 for ureq: keep-alive, one request at a time, Content-Length bodies.
    fn serve(mut stream: TcpStream, handler: Handler, log: Arc<Mutex<Vec<Req>>>) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let mut buf = Vec::new();
        loop {
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i;
                }
                if !fill(&mut stream, &mut buf) {
                    return;
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let mut lines = head.lines();
            let path = lines.next().and_then(|l| l.split(' ').nth(1)).unwrap_or("").to_owned();
            let (mut len, mut auth) = (0usize, None);
            for line in lines {
                let Some((name, value)) = line.split_once(':') else { continue };
                match name.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = value.trim().parse().unwrap_or(0),
                    "authorization" => auth = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
            let body_at = head_end + 4;
            while buf.len() < body_at + len {
                if !fill(&mut stream, &mut buf) {
                    return;
                }
            }
            let body = String::from_utf8_lossy(&buf[body_at..body_at + len]).into_owned();
            buf.drain(..body_at + len);
            let req = Req { path, auth, body };
            log.lock().unwrap().push(req.clone());
            let (status, text) = handler(&req);
            let response = format!("HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n{text}", text.len());
            if stream.write_all(response.as_bytes()).is_err() {
                return;
            }
        }
    }

    fn connect_fake(fake: &FakeRelay, role: &str) -> (Conn, usize, Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let (conn, seat) = connect_to(&fake.server(), "ABCD", role, &who(), tx).unwrap_or_else(|e| panic!("connect: {e}"));
        (conn, seat, rx)
    }

    fn next(rx: &Receiver<Event>) -> Event {
        rx.recv_timeout(WAIT).expect("an event within the wait")
    }

    fn disconnected(rx: &Receiver<Event>) -> String {
        match next(rx) {
            Event::Disconnected(why) => why,
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    fn nothing_more(rx: &Receiver<Event>) {
        if let Ok(e) = rx.recv_timeout(QUIET) {
            panic!("unexpected event {e:?}");
        }
    }

    // ---- one request -------------------------------------------------------------------

    #[test]
    fn post_returns_any_answer_and_carries_the_bearer_token_only_once_it_has_one() {
        let fake = FakeRelay::start(|req| match req.auth {
            Some(_) => (204, String::new()),
            None => (401, "not in this room".into()),
        });
        assert_eq!(fake.relay("").post("send?id=1", "hello", QUICK_TIMEOUT), Ok((401, "not in this room".into())));
        assert_eq!(fake.relay("t0k").post("send?id=1", "hello", QUICK_TIMEOUT), Ok((204, String::new())));
        let seen = fake.seen();
        assert_eq!(paths(&seen), ["/room/ABCD/send?id=1", "/room/ABCD/send?id=1"]);
        assert_eq!((seen[0].auth.as_deref(), seen[0].body.as_str()), (None, "hello"));
        assert_eq!((seen[1].auth.as_deref(), seen[1].body.as_str()), (Some("Bearer t0k"), "hello"));
    }

    #[test]
    fn post_with_nobody_listening_is_an_err() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // released at once
        let relay = Relay { agent: agent(), room: format!("http://127.0.0.1:{port}/room/ABCD"), token: String::new() };
        assert!(relay.post("poll?after=0", "", QUICK_TIMEOUT).is_err());
    }

    #[test]
    fn post_retrying_takes_a_4xx_as_the_final_word() {
        let fake = FakeRelay::start(|_| (409, "room full".into()));
        assert_eq!(fake.relay("").post_retrying("host", "", QUICK_TIMEOUT), Ok((409, "room full".into())));
        assert_eq!(fake.seen().len(), 1, "no retry");
    }

    #[test]
    fn post_retrying_outlasts_a_5xx() {
        let hits = Arc::new(AtomicUsize::new(0));
        let fake = FakeRelay::start({
            let hits = hits.clone();
            move |_| match hits.fetch_add(1, Ordering::SeqCst) {
                0 => (503, "busy".into()),
                _ => (200, "fine".into()),
            }
        });
        let started = Instant::now();
        assert_eq!(fake.relay("").post_retrying("poll?after=0", "", QUICK_TIMEOUT), Ok((200, "fine".into())));
        assert_eq!(fake.seen().len(), 2);
        assert!(started.elapsed() >= RETRY_EVERY, "waits between attempts");
    }

    // ---- connecting --------------------------------------------------------------------

    #[test]
    fn connect_opens_the_room_then_polls_with_the_token_it_was_given() {
        let fake = FakeRelay::scripted(ticket(201, "secret", 0), vec![], sent());
        let (_conn, seat, _rx) = connect_fake(&fake, "host");
        assert_eq!(seat, DEALER);
        let seen = fake.wait_for(2);
        assert_eq!(paths(&seen), ["/room/ABCD/host", "/room/ABCD/poll?after=0"]);
        assert_eq!((seen[0].auth.as_deref(), seen[0].body.as_str()), (None, r#"{"id":"0123456789abcdef0123456789abcdef","name":"tester"}"#), "who we are goes with the opening request");
        assert_eq!(seen[1].auth.as_deref(), Some("Bearer secret"));
    }

    #[test]
    fn joining_is_the_same_dance_with_a_200_and_a_seat_number() {
        let fake = FakeRelay::scripted(ticket(200, "j", 7), vec![], sent());
        let (_conn, seat, _rx) = connect_fake(&fake, "join");
        assert_eq!(seat, 7);
        let seen = fake.wait_for(2);
        assert_eq!(paths(&seen), ["/room/ABCD/join", "/room/ABCD/poll?after=0"]);
        assert_eq!(seen[0].body, serde_json::to_string(&who()).unwrap());
        assert_eq!(seen[1].auth.as_deref(), Some("Bearer j"));
    }

    #[test]
    fn a_seat_that_makes_no_sense_for_the_role_is_a_bad_reply() {
        for (role, seat) in [("host", 1), ("join", 0), ("join", MAX_PLAYERS + 1), ("join", 500)] {
            let fake = FakeRelay::scripted(ticket(200, "t", seat), vec![], sent());
            let (tx, _rx) = mpsc::channel();
            let err = connect_to(&fake.server(), "ABCD", role, &who(), tx).err().unwrap_or_else(|| panic!("{role} in seat {seat} accepted"));
            assert_eq!(err.to_string(), "bad reply from relay");
            assert_eq!(fake.seen().len(), 1, "nothing is polled");
        }
    }

    #[test]
    fn every_player_seat_is_accepted() {
        for seat in 1..=MAX_PLAYERS {
            let fake = FakeRelay::scripted(ticket(200, "t", seat), vec![], sent());
            let (_conn, got, _rx) = connect_fake(&fake, "join");
            assert_eq!(got, seat);
        }
    }

    #[test]
    fn connect_passes_on_the_relays_refusal_word_for_word() {
        for (status, why) in [(409, "room full (9 players)"), (404, "no such room (check the code, or the host quit)")] {
            let fake = FakeRelay::scripted((status, why.into()), vec![], sent());
            let (tx, _rx) = mpsc::channel();
            let err = connect_to(&fake.server(), "ABCD", "join", &who(), tx).err().expect("refused");
            assert_eq!(err.to_string(), why);
            assert_eq!(fake.seen().len(), 1, "a refusal is not retried and nothing is polled");
        }
    }

    #[test]
    fn connect_rejects_a_relay_that_does_not_speak_json_or_leaves_out_the_seat() {
        for body in ["<html>proxy login</html>", r#"{"token":"old-relay"}"#] {
            let fake = FakeRelay::scripted((200, body.into()), vec![], sent());
            let (tx, _rx) = mpsc::channel();
            let err = connect_to(&fake.server(), "ABCD", "host", &who(), tx).err().expect("refused");
            assert_eq!(err.to_string(), "bad reply from relay", "{body}");
        }
    }

    #[test]
    fn connect_names_an_unreachable_relay() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (tx, _rx) = mpsc::channel();
        let err = connect_to(&format!("http://127.0.0.1:{port}"), "ABCD", "host", &who(), tx).err().expect("unreachable");
        assert!(err.to_string().starts_with("cannot reach relay: "), "{err}");
    }

    #[test]
    fn connect_accepts_ws_urls_and_trailing_slashes() {
        let fake = FakeRelay::scripted(open_ok(), vec![], sent());
        let (tx, _rx) = mpsc::channel();
        let _conn = connect_to(&format!("ws://127.0.0.1:{}/", fake.port), "ABCD", "host", &who(), tx).unwrap();
        assert_eq!(fake.wait_for(1)[0].path, "/room/ABCD/host");
    }

    // ---- polling -----------------------------------------------------------------------

    #[test]
    fn polls_deliver_seat_changes_and_messages_in_order_and_ack_what_was_seen() {
        let hit = msg(Msg::Action(Action::Hit));
        let stand = msg(Msg::Action(Action::Stand));
        let fake = FakeRelay::scripted(
            open_ok(),
            vec![
                inbox(&[(1, 3, "joined".into()), (2, 3, hit.clone()), (3, 5, "joined".into())]),
                inbox(&[(3, 5, "joined".into()), (4, 5, stand), (5, 3, "left".into())]), // the relay resends 3 until it is acked
            ],
            sent(),
        );
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert!(matches!(next(&rx), Event::Joined(3, None)), "a joiner who gave no name");
        assert!(matches!(next(&rx), Event::Net(3, Msg::Action(Action::Hit))));
        assert!(matches!(next(&rx), Event::Joined(5, None)));
        assert!(matches!(next(&rx), Event::Net(5, Msg::Action(Action::Stand))));
        assert!(matches!(next(&rx), Event::Left(3)));
        nothing_more(&rx); // the resent event is not replayed
        let polls = fake.wait_for_verb("poll", 3);
        assert_eq!(paths(&polls)[..3], ["/room/ABCD/poll?after=0", "/room/ABCD/poll?after=3", "/room/ABCD/poll?after=5"]);
    }

    #[test]
    fn a_joiner_arrives_with_the_name_they_gave_the_relay() {
        let unnamed = (200, serde_json::json!({ "events": [{ "seq": 2, "from": 4, "text": "joined", "name": "" }] }).to_string());
        let fake = FakeRelay::scripted(open_ok(), vec![joined_as(1, 3, "bob"), unnamed], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        match next(&rx) {
            Event::Joined(3, Some(name)) => assert_eq!(name, "bob"),
            other => panic!("expected player 3 as bob, got {other:?}"),
        }
        assert!(matches!(next(&rx), Event::Joined(4, None)), "an empty name is no name");
    }

    #[test]
    fn a_state_snapshot_comes_through_whole_and_from_the_dealer() {
        let mut g = GameState::new();
        g.round = 7;
        g.sit(4);
        g.seats[DEALER].as_mut().unwrap().hand = vec![Card { rank: 12, suit: 3 }, Card { rank: 1, suit: 0 }];
        g.phase = Phase::DealerTurn;
        let fake = FakeRelay::scripted(ticket(200, "j", 4), vec![inbox(&[(1, DEALER, msg(Msg::State(Box::new(g.clone()))))])], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "join");
        match next(&rx) {
            Event::Net(from, Msg::State(s)) => assert_eq!((from, *s), (DEALER, g)),
            other => panic!("expected the snapshot, got {other:?}"),
        }
    }

    #[test]
    fn the_relay_closing_the_room_ends_the_game_in_its_own_words() {
        let fake = FakeRelay::scripted(open_ok(), vec![(410, "host quit".into())], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert_eq!(disconnected(&rx), "host quit");
        nothing_more(&rx);
    }

    #[test]
    fn a_bare_error_status_is_still_explained() {
        let fake = FakeRelay::scripted(open_ok(), vec![(404, String::new())], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert_eq!(disconnected(&rx), "relay said 404");
    }

    #[test]
    fn a_poll_answer_that_is_not_json_disconnects() {
        let fake = FakeRelay::scripted(open_ok(), vec![(200, "<html>".into())], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert_eq!(disconnected(&rx), "bad reply from relay");
    }

    #[test]
    fn an_envelope_without_a_sender_is_a_bad_reply() {
        // an old relay that does not number seats
        let old = (200, serde_json::json!({ "events": [{ "seq": 1, "text": "joined" }] }).to_string());
        let fake = FakeRelay::scripted(open_ok(), vec![old], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert_eq!(disconnected(&rx), "bad reply from relay");
    }

    #[test]
    fn an_unreadable_message_disconnects() {
        let fake = FakeRelay::scripted(open_ok(), vec![inbox(&[(1, 2, r#"{"Nope":1}"#.into())])], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert_eq!(disconnected(&rx), "unreadable game message (is everyone on the same version?)");
    }

    #[test]
    fn a_flaky_poll_is_retried_not_fatal() {
        let fake = FakeRelay::scripted(open_ok(), vec![(502, String::new()), inbox(&[(1, 1, "joined".into())])], sent());
        let (_conn, _seat, rx) = connect_fake(&fake, "host");
        assert!(matches!(next(&rx), Event::Joined(1, None)));
        let polls = fake.wait_for_verb("poll", 2);
        assert_eq!(paths(&polls)[..2], ["/room/ABCD/poll?after=0", "/room/ABCD/poll?after=0"]);
    }

    // ---- sending -----------------------------------------------------------------------

    #[test]
    fn sends_are_posted_in_order_with_a_serial_and_the_token() {
        let fake = FakeRelay::scripted(open_ok(), vec![], sent());
        let (conn, _seat, rx) = connect_fake(&fake, "host");
        let mut g = GameState::new();
        g.round = 1;
        conn.send(Msg::Action(Action::Ready));
        conn.send(Msg::State(Box::new(g.clone())));
        let sends = fake.wait_for_verb("send", 2);
        assert_eq!(paths(&sends), ["/room/ABCD/send?id=1", "/room/ABCD/send?id=2"]);
        assert_eq!(sends[0].body, msg(Msg::Action(Action::Ready)));
        assert_eq!(sends[1].body, msg(Msg::State(Box::new(g))));
        assert!(sends.iter().all(|r| r.auth.as_deref() == Some("Bearer tok")));
        nothing_more(&rx); // a 204 is silent
    }

    fn report(round: u32) -> Report {
        Report { round, results: vec![HandResult { seat: 1, outcome: Outcome::Win }] }
    }

    #[test]
    fn a_report_is_posted_to_result_in_turn_with_the_token_and_gets_no_verdict() {
        let fake = FakeRelay::scripted(open_ok(), vec![], sent());
        let (conn, _seat, rx) = connect_fake(&fake, "host");
        let g = GameState::new();
        conn.send(Msg::State(Box::new(g.clone())));
        conn.report(report(1));
        conn.send(Msg::Action(Action::Ready));
        let outgoing = fake.wait_until(|seen| seen.iter().filter(|r| matches!(verb(&r.path), "send" | "result")).count() >= 3, "three posts");
        let outgoing: Vec<&Req> = outgoing.iter().filter(|r| matches!(verb(&r.path), "send" | "result")).collect();
        assert_eq!(paths(&outgoing.iter().map(|r| (*r).clone()).collect::<Vec<_>>()), ["/room/ABCD/send?id=1", "/room/ABCD/result", "/room/ABCD/send?id=2"], "in the order queued; reports take no serial");
        assert_eq!(outgoing[1].body, serde_json::to_string(&report(1)).unwrap());
        assert_eq!(outgoing[1].auth.as_deref(), Some("Bearer tok"));
        nothing_more(&rx);
    }

    #[test]
    fn a_relay_that_refuses_reports_costs_nothing() {
        // an older relay, or one without a scoreboard: 404 on result, the game goes on
        let fake = FakeRelay::scripted_with_results(open_ok(), vec![], sent(), (404, "usage: POST /room/<CODE>/{host|join|send|poll|leave}".into()));
        let (conn, _seat, rx) = connect_fake(&fake, "host");
        conn.report(report(1));
        conn.send(Msg::Action(Action::Ready));
        let sends = fake.wait_for_verb("send", 1);
        assert_eq!(paths(&sends), ["/room/ABCD/send?id=1"], "the send behind the refused report still goes out");
        assert_eq!(fake.wait_for_verb("result", 1).len(), 1, "a refused report is not retried");
        nothing_more(&rx);
    }

    #[test]
    fn a_report_goes_out_before_leave() {
        let fake = FakeRelay::scripted(open_ok(), vec![], sent());
        let (conn, _seat, _rx) = connect_fake(&fake, "host");
        conn.report(report(2));
        conn.close();
        let seen = fake.seen();
        let outgoing: Vec<&str> = paths(&seen).into_iter().filter(|p| matches!(verb(p), "result" | "leave")).collect();
        assert_eq!(outgoing, ["/room/ABCD/result", "/room/ABCD/leave"]);
    }

    // ---- the leaderboard ------------------------------------------------------------------

    #[test]
    fn scores_posts_our_id_and_reads_the_rows_back() {
        let rows = serde_json::json!({ "rows": [
            { "name": "alice", "wins": 12, "losses": 8, "pushes": 2, "hands": 22, "you": false },
            { "name": "tester", "wins": 1, "losses": 0, "pushes": 0, "hands": 1, "you": true },
        ] });
        let fake = FakeRelay::start(move |req| match req.path.as_str() {
            "/scores" => (200, rows.to_string()),
            _ => (404, "unknown action".into()),
        });
        let rows = scores_from(&fake.server(), &who()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].name.as_str(), rows[0].wins, rows[0].you), ("alice", 12, false));
        assert_eq!((rows[1].name.as_str(), rows[1].hands, rows[1].you), ("tester", 1, true));
        let seen = fake.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0].auth.as_deref(), seen[0].body.as_str()), (None, r#"{"id":"0123456789abcdef0123456789abcdef"}"#));
    }

    #[test]
    fn scores_passes_on_a_refusal_and_rejects_non_json() {
        let fake = FakeRelay::start(|_| (501, "this relay keeps no scores".into()));
        assert_eq!(scores_from(&fake.server(), &who()).unwrap_err().to_string(), "this relay keeps no scores");
        let fake = FakeRelay::start(|_| (200, "<html>".into()));
        assert_eq!(scores_from(&fake.server(), &who()).unwrap_err().to_string(), "bad reply from relay");
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let err = scores_from(&format!("http://127.0.0.1:{port}"), &who()).unwrap_err().to_string();
        assert!(err.starts_with("cannot reach relay: "), "{err}");
    }

    #[test]
    fn a_refused_send_ends_the_game() {
        let fake = FakeRelay::scripted(ticket(200, "j", 1), vec![], (401, "not in this room".into()));
        let (conn, _seat, rx) = connect_fake(&fake, "join");
        conn.send(Msg::Action(Action::Hit));
        assert_eq!(disconnected(&rx), "not in this room");
    }

    #[test]
    fn close_flushes_pending_sends_then_tells_the_relay_we_left() {
        let fake = FakeRelay::scripted(open_ok(), vec![], sent());
        let (conn, _seat, _rx) = connect_fake(&fake, "host");
        conn.send(Msg::Action(Action::Stand));
        let started = Instant::now();
        conn.close();
        assert!(started.elapsed() < QUICK_TIMEOUT, "close returns as soon as the relay has been told");
        let seen = fake.seen();
        let outgoing: Vec<&str> = paths(&seen).into_iter().filter(|p| matches!(verb(p), "send" | "leave")).collect();
        assert_eq!(outgoing, ["/room/ABCD/send?id=1", "/room/ABCD/leave"]);
        let leave = seen.iter().find(|r| verb(&r.path) == "leave").unwrap();
        assert_eq!(leave.auth.as_deref(), Some("Bearer tok"));
    }
}
