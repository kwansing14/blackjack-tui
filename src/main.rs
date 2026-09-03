mod game;
mod net;

use game::{hand_value, Action, Card, GameState, Outcome, Phase};
use net::{Event, Msg};
use std::error::Error;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;

fn usage() -> ! {
    eprintln!("usage: blackjack host [port]\n       blackjack join <ip:port>");
    std::process::exit(2);
}

fn cards(hand: &[Card]) -> String {
    hand.iter().map(Card::label).collect::<Vec<_>>().join(" ")
}

fn render(state: &GameState, me: usize) -> String {
    let mut out = format!("\n===== Round {} =====\n", state.round);
    let dealer = match state.phase {
        Phase::WaitingForReady => "(no cards yet)".to_string(),
        Phase::PlayerTurn(_) => format!("{} ??", state.dealer[0].label()),
        Phase::RoundOver => format!("{}  = {}", cards(&state.dealer), hand_value(&state.dealer)),
    };
    out += &format!("Dealer:   {dealer}\n");
    for (i, hand) in state.players.iter().enumerate() {
        let marker = if state.phase == Phase::PlayerTurn(i) { ">" } else { " " };
        let you = if i == me { " (you)" } else { "      " };
        out += &format!("{marker} Player {}{you}: {}", i + 1, cards(hand));
        if !hand.is_empty() {
            let v = hand_value(hand);
            out += &format!("  = {v}");
            if v > 21 {
                out += "  BUST";
            }
        }
        if let Some(r) = &state.results {
            out += match r[i] {
                Outcome::Win => "  WIN",
                Outcome::Lose => "  LOSE",
                Outcome::Push => "  PUSH",
            };
        }
        if matches!(state.phase, Phase::WaitingForReady | Phase::RoundOver) {
            out += if state.ready[i] { "  [ready]" } else { "  [not ready]" };
        }
        out.push('\n');
    }
    out += match state.phase {
        Phase::PlayerTurn(p) if p == me => "Your turn: (h)it, (s)tand, (q)uit",
        Phase::PlayerTurn(_) => "Waiting for other player... (q)uit",
        _ => "(r)eady for next round, (q)uit",
    };
    out += "\n> ";
    out
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut stream, me) = match args.first().map(String::as_str) {
        Some("host") => {
            let port = args.get(1).map(String::as_str).unwrap_or("7878");
            let listener = TcpListener::bind(format!("0.0.0.0:{port}"))?;
            // ponytail: UDP connect sends nothing; just makes the OS pick the outbound interface
            let ip = std::net::UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| s.connect("8.8.8.8:80").and_then(|_| s.local_addr()))
                .map(|a| a.ip().to_string())
                .unwrap_or_else(|_| "<your-ip>".into());
            println!("waiting for player 2 on port {port} ...");
            println!("player 2 runs:  blackjack join {ip}:{port}");
            let (stream, peer) = listener.accept()?;
            println!("player 2 connected from {peer}");
            (stream, 0usize)
        }
        Some("join") => {
            let addr = args.get(1).unwrap_or_else(|| usage());
            (TcpStream::connect(addr)?, 1usize)
        }
        _ => usage(),
    };
    stream.set_nodelay(true)?;
    let is_host = me == 0;

    let (tx, rx) = mpsc::channel();
    net::spawn_reader(stream.try_clone()?, tx.clone());
    net::spawn_input(tx);

    let mut state = GameState::new();
    let mut shoe = Vec::new();
    if is_host {
        net::send(&mut stream, &Msg::State(state.clone()))?;
    }
    print!("{}", render(&state, me));
    std::io::stdout().flush()?;

    loop {
        let mut local_action = None;
        let mut changed = false;
        match rx.recv()? {
            Event::Line(line) => match line.trim() {
                "q" | "quit" => return Ok(()),
                "h" | "hit" => local_action = Some(Action::Hit),
                "s" | "stand" => local_action = Some(Action::Stand),
                "r" | "ready" => local_action = Some(Action::Ready),
                "" => {}
                other => println!("unknown command: {other:?}  (h/s/r/q)"),
            },
            Event::Net(Msg::Action(a)) if is_host => {
                state.apply(1, a, &mut shoe);
                net::send(&mut stream, &Msg::State(state.clone()))?;
                changed = true;
            }
            Event::Net(Msg::State(s)) if !is_host => {
                state = s;
                changed = true;
            }
            Event::Net(_) => {} // wrong direction, ignore
            Event::Disconnected => {
                println!("\nother player disconnected, bye");
                return Ok(());
            }
        }

        if let Some(a) = local_action {
            if is_host {
                state.apply(0, a, &mut shoe);
                net::send(&mut stream, &Msg::State(state.clone()))?;
                changed = true;
            } else {
                net::send(&mut stream, &Msg::Action(a))?;
                // client waits for the host's State snapshot before redrawing
            }
        }

        if changed {
            print!("{}", render(&state, me));
            std::io::stdout().flush()?;
        }
    }
}
