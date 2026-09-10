//! Drives the built `blackjack` binary the way a player would, against a stand-in relay on
//! localhost. Nothing here touches the network; the real relay is covered by `e2e.py`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

static SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// A profile path nothing else uses, so tests never touch ~/.config or each other.
fn scratch_profile() -> PathBuf {
    let n = SCRATCH.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("blackjack-cli-test-{}-{n}", std::process::id())).join("player.json")
}

/// Runs the binary as player `tester` with a throwaway profile. See `run`.
fn blackjack(args: &[&str], server: &str) -> Output {
    run(args, server, Some("tester"), &scratch_profile())
}

/// Runs the binary to completion with stdin closed, or kills it after 20s so a hang fails
/// instead of stalling the suite. Proxy variables are cleared so ureq talks to localhost directly.
/// `name` goes in BLACKJACK_NAME (none means the binary must manage without one).
fn run(args: &[&str], server: &str, name: Option<&str>, profile: &Path) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blackjack"));
    cmd.args(args)
        .env("BLACKJACK_SERVER", server)
        .env("BLACKJACK_PROFILE", profile)
        .env_remove("BLACKJACK_NAME")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(name) = name {
        cmd.env("BLACKJACK_NAME", name);
    }
    let mut child = cmd.spawn().expect("spawn blackjack");
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("wait on blackjack").is_none() {
        if Instant::now() > deadline {
            child.kill().ok();
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("collect output")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A URL nothing listens on.
fn dead_server() -> String {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    format!("http://127.0.0.1:{port}")
}

struct FakeRelay {
    url: String,
    paths: Arc<Mutex<Vec<String>>>,
}

impl FakeRelay {
    /// Answers host/join with `open`. The first poll gets `first_poll` if there is one; every
    /// poll after that is told the host quit, which makes the client exit. That answer is held
    /// briefly, like a real relay's, so anything the client sent in reaction to the first poll
    /// gets out before it leaves. `/scores` is refused like an old relay would.
    fn start(open: (u16, &str), first_poll: Option<String>) -> Self {
        Self::start_with(open, first_poll, None)
    }

    /// `start`, with `/scores` answered by `scores` when given.
    fn start_with(open: (u16, &str), first_poll: Option<String>, scores: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let paths: Arc<Mutex<Vec<String>>> = Arc::default();
        let open = (open.0, open.1.to_owned());
        let polls = Arc::new(AtomicUsize::new(0));
        let first_poll = Arc::new(first_poll);
        let scores = Arc::new(scores);
        let log = paths.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (open, first_poll, polls, scores, log) = (open.clone(), first_poll.clone(), polls.clone(), scores.clone(), log.clone());
                thread::spawn(move || {
                    serve(stream, log, move |path| {
                        let verb = path.rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("");
                        match verb {
                            "host" | "join" => open.clone(),
                            "poll" => match (polls.fetch_add(1, Ordering::SeqCst), first_poll.as_deref()) {
                                (0, Some(inbox)) => (200, inbox.to_owned()),
                                _ => {
                                    thread::sleep(Duration::from_millis(300));
                                    (410, "host quit".to_owned())
                                }
                            },
                            "scores" => match scores.as_deref() {
                                Some(body) => (200, body.to_owned()),
                                None => (404, "usage: POST /room/<CODE>/{host|join|send|poll|leave}".to_owned()),
                            },
                            _ => (204, String::new()),
                        }
                    })
                });
            }
        });
        FakeRelay { url, paths }
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
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
fn serve(mut stream: TcpStream, log: Arc<Mutex<Vec<String>>>, reply: impl Fn(&str) -> (u16, String)) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
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
        let path = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("").to_owned();
        let len: usize = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(0);
        let end = head_end + 4 + len;
        while buf.len() < end {
            if !fill(&mut stream, &mut buf) {
                return;
            }
        }
        buf.drain(..end);
        log.lock().unwrap().push(path.clone());
        let (status, body) = reply(&path);
        let response = format!("HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        if stream.write_all(response.as_bytes()).is_err() {
            return;
        }
    }
}

/// A table snapshot as the host would send it: the dealer in hearts, player `seat` in spades.
fn snapshot(seat: usize, player: &[u8], dealer: &[u8], phase: &str, turn: usize) -> String {
    let card = |rank: &u8, suit: u8| serde_json::json!({ "rank": rank, "suit": suit });
    let mut seats = vec![serde_json::Value::Null; 10];
    seats[0] = serde_json::json!({ "hand": dealer.iter().map(|r| card(r, 1)).collect::<Vec<_>>(), "ready": false, "result": null });
    seats[seat] = serde_json::json!({ "hand": player.iter().map(|r| card(r, 0)).collect::<Vec<_>>(), "ready": false, "result": null });
    serde_json::json!({ "State": { "seats": seats, "phase": phase, "turn": turn, "round": 1 } }).to_string()
}

fn inbox(events: &[(u64, usize, &str)]) -> String {
    let events: Vec<_> = events.iter().map(|(seq, from, text)| serde_json::json!({ "seq": seq, "from": from, "text": text })).collect();
    serde_json::json!({ "events": events }).to_string()
}

/// An inbox with one "joined" event that carries the newcomer's name.
fn joined_as(seq: u64, from: usize, name: &str) -> String {
    serde_json::json!({ "events": [{ "seq": seq, "from": from, "text": "joined", "name": name }] }).to_string()
}

// ---- argument handling ---------------------------------------------------------------------

#[test]
fn no_arguments_prints_usage_and_exits_2() {
    let out = blackjack(&[], &dead_server());
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("usage: blackjack host"), "{err}");
    assert!(err.contains("blackjack join <CODE>"), "{err}");
    assert!(out.stdout.is_empty());
}

#[test]
fn join_without_a_code_prints_usage() {
    let out = blackjack(&["join"], &dead_server());
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("usage:"));
}

#[test]
fn an_unknown_command_prints_usage() {
    let out = blackjack(&["deal"], &dead_server());
    assert_eq!(out.status.code(), Some(2));
    let err = text(&out.stderr);
    assert!(err.contains("usage:"), "{err}");
    assert!(err.contains("blackjack scores") && err.contains("blackjack name [NEW]"), "{err}");
}

// ---- who you are -----------------------------------------------------------------------------

#[test]
fn with_no_name_and_no_terminal_the_game_says_what_to_do_and_never_reaches_the_relay() {
    let relay = FakeRelay::start((201, r#"{"token":"h","seat":0}"#), None);
    let out = run(&["host"], &relay.url, None, &scratch_profile());
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("no name given; set BLACKJACK_NAME or run: blackjack name <NAME>"), "{err}");
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(relay.paths().is_empty(), "no room is opened without a name");
}

#[test]
fn name_shows_sets_and_keeps_the_saved_profile() {
    let profile = scratch_profile();
    let server = dead_server(); // never contacted

    let out = run(&["name"], &server, None, &profile);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).starts_with("no name yet;"), "{}", text(&out.stdout));

    let out = run(&["name", "  Zed "], &server, None, &profile);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("you are now Zed ({})\n", profile.display()));
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&profile).unwrap()).unwrap();
    assert_eq!(saved["name"], "Zed");
    let id = saved["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 32);

    let out = run(&["name"], &server, None, &profile);
    assert_eq!(text(&out.stdout), format!("you are Zed ({})\n", profile.display()));

    let out = run(&["name", "Zoe"], &server, None, &profile);
    assert!(out.status.success());
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&profile).unwrap()).unwrap();
    assert_eq!((saved["name"].as_str(), saved["id"].as_str()), (Some("Zoe"), Some(id.as_str())), "a rename keeps the id");

    let out = run(&["name", ""], &server, None, &profile);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("a name is 1 to 16 printable characters"), "{}", text(&out.stderr));

    let out = run(&["host"], &server, None, &profile);
    assert!(text(&out.stderr).contains("cannot reach relay"), "a saved name means no prompt: {}", text(&out.stderr));
}

// ---- the leaderboard ---------------------------------------------------------------------------

#[test]
fn scores_prints_the_leaderboard_with_our_row_marked() {
    let rows = serde_json::json!({ "rows": [
        { "name": "alice", "wins": 12, "losses": 8, "pushes": 2, "hands": 22, "you": false },
        { "name": "tester", "wins": 1, "losses": 0, "pushes": 0, "hands": 1, "you": true },
    ] });
    let relay = FakeRelay::start_with((201, ""), None, Some(rows.to_string()));
    let out = blackjack(&["scores"], &relay.url);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "  #  name       W    L    P  hands\n  1  alice     12    8    2     22\n  2  tester     1    0    0      1  (you)\n"
    );
    assert_eq!(relay.paths(), ["/scores"]);
}

#[test]
fn scores_against_a_relay_without_a_scoreboard_fails_in_its_words() {
    let relay = FakeRelay::start((201, ""), None);
    let out = blackjack(&["scores"], &relay.url);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("usage: POST /room/<CODE>/"), "{}", text(&out.stderr));
}

// ---- talking to the relay ------------------------------------------------------------------

#[test]
fn hosting_with_no_relay_says_so_and_fails() {
    let out = blackjack(&["host"], &dead_server());
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("cannot reach relay"), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty(), "no room is announced");
}

#[test]
fn joining_a_missing_room_shows_the_relays_reason_and_uppercases_the_code() {
    let relay = FakeRelay::start((404, "no such room (check the code, or the host quit)"), None);
    let out = blackjack(&["join", "kqzp"], &relay.url);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no such room"), "{}", text(&out.stderr));
    assert_eq!(relay.paths(), ["/room/KQZP/join"]);
}

#[test]
fn joining_a_full_room_is_refused_in_the_relays_words() {
    let relay = FakeRelay::start((409, "room full (9 players)"), None);
    let out = blackjack(&["join", "ABCD"], &relay.url);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("room full (9 players)"), "{}", text(&out.stderr));
}

#[test]
fn a_relay_that_does_not_number_seats_is_rejected() {
    let relay = FakeRelay::start((200, r#"{"token":"j"}"#), None);
    let out = blackjack(&["join", "ABCD"], &relay.url);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("bad reply from relay"), "{}", text(&out.stderr));
}

#[test]
fn the_host_announces_the_room_shows_the_table_and_leaves_when_the_relay_closes_it() {
    let relay = FakeRelay::start((201, r#"{"token":"h","seat":0}"#), None);
    let out = blackjack(&["host"], &relay.url);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", text(&out.stderr));

    let code = stdout.split("room ").nth(1).and_then(|s| s.get(..4)).expect("a room code");
    assert!(code.chars().all(|c| c.is_ascii_uppercase()), "code {code:?}");
    assert!(stdout.contains(&format!("room {code} open, waiting for players (up to 9) ...")), "{stdout}");
    assert!(stdout.contains(&format!("players run:  blackjack join {code}")), "{stdout}");
    assert!(stdout.contains("===== Round 0 ====="), "{stdout}");
    assert!(stdout.contains("  Dealer (you)  :   [not ready]"), "{stdout}");
    assert!(!stdout.contains("Player"), "nobody has joined yet:\n{stdout}");
    assert!(stdout.contains("(r)eady for next round, (q)uit"), "{stdout}");
    assert!(stdout.ends_with("host quit, bye\n"), "{stdout}");

    let paths = relay.paths();
    assert_eq!(paths[0], format!("/room/{code}/host"));
    assert!(paths.contains(&format!("/room/{code}/poll?after=0")), "{paths:?}");
}

#[test]
fn the_host_seats_a_joiner_and_sends_them_the_table() {
    let relay = FakeRelay::start((201, r#"{"token":"h","seat":0}"#), Some(inbox(&[(1, 3, "joined")])));
    let out = blackjack(&["host"], &relay.url);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", text(&out.stderr));
    assert!(stdout.contains("player 3 joined\n"), "{stdout}");
    assert!(stdout.contains("  Player 3      :   [not ready]\n"), "a nameless joiner has no tally to show: {stdout}");
    assert!(stdout.contains("  Dealer (you)  :   [not ready]  tester 0W 0L 0P\n"), "the host sits down under their own name: {stdout}");

    let paths = relay.paths();
    let sends: Vec<&String> = paths.iter().filter(|p| p.contains("/send?")).collect();
    assert_eq!(sends.len(), 1, "one snapshot goes out for the newcomer: {paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("/poll?after=1")), "the joined event is acked: {paths:?}");
}

#[test]
fn the_host_greets_a_named_joiner_and_shows_their_tally() {
    let relay = FakeRelay::start((201, r#"{"token":"h","seat":0}"#), Some(joined_as(1, 3, "bob")));
    let out = blackjack(&["host"], &relay.url);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", text(&out.stderr));
    assert!(stdout.contains("player 3 (bob) joined\n"), "{stdout}");
    assert!(stdout.contains("  Player 3      :   [not ready]  bob 0W 0L 0P\n"), "{stdout}");
}

#[test]
fn the_joiner_draws_the_table_from_the_hosts_first_snapshot() {
    let state = snapshot(4, &[10, 7], &[9, 5], "PlayerTurn", 4);
    let relay = FakeRelay::start((200, r#"{"token":"j","seat":4}"#), Some(inbox(&[(1, 0, &state)])));
    let out = blackjack(&["join", "ABCD"], &relay.url);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", text(&out.stderr));

    assert!(stdout.starts_with("joined room ABCD as player 4, waiting for the dealer ...\n"), "{stdout}");
    assert!(!stdout.contains("Round 0"), "the joiner waits for the host's state before drawing:\n{stdout}");
    assert!(stdout.contains("===== Round 1 ====="), "{stdout}");
    assert!(stdout.contains("  Dealer        : 9♥ ??"), "{stdout}");
    assert!(stdout.contains("> Player 4 (you): 10♠ 7♠  = 17"), "{stdout}");
    assert!(stdout.contains("Your turn: (h)it, (s)tand, (q)uit"), "{stdout}");
    assert!(stdout.ends_with("host quit, bye\n"), "{stdout}");

    let paths = relay.paths();
    assert_eq!(paths[0], "/room/ABCD/join");
    assert!(paths.contains(&"/room/ABCD/poll?after=1".to_owned()), "the snapshot is acked: {paths:?}");
}
