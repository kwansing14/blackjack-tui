mod game;
mod net;

use game::{hand_value, Action, Card, GameState, Outcome, Phase};
use net::{Event, Msg};
use std::error::Error;
use std::io::Write;
use std::sync::mpsc;

fn usage() -> ! {
    eprintln!("usage: blackjack host\n       blackjack join <CODE>");
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
        let marker = if state.phase == Phase::PlayerTurn(i) {
            ">"
        } else {
            " "
        };
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
            out += if state.ready[i] {
                "  [ready]"
            } else {
                "  [not ready]"
            };
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
    let (tx, rx) = mpsc::channel();
    let (out, me) = match args.first().map(String::as_str) {
        Some("host") => {
            let code: String = (0..4)
                .map(|_| (b'A' + rand::random_range(0..26u8)) as char)
                .collect();
            let out = net::connect(&code, "host", tx.clone())?;
            println!("room {code} open, waiting for player 2 ...");
            println!("player 2 runs:  blackjack join {code}");
            (out, 0usize)
        }
        Some("join") => {
            let code = args.get(1).unwrap_or_else(|| usage()).to_uppercase();
            (net::connect(&code, "join", tx.clone())?, 1usize)
        }
        _ => usage(),
    };
    let is_host = me == 0;
    net::spawn_input(tx);

    let mut state = GameState::new();
    let mut shoe = Vec::new();
    print!("{}", render(&state, me));
    std::io::stdout().flush()?;

    loop {
        let mut local_action = None;
        let mut changed = false;
        match rx.recv()? {
            Event::Line(line) => match line.trim() {
                "q" | "quit" => {
                    drop(out);
                    std::thread::sleep(std::time::Duration::from_millis(100)); // let the socket thread send Close
                    return Ok(());
                }
                "h" | "hit" => local_action = Some(Action::Hit),
                "s" | "stand" => local_action = Some(Action::Stand),
                "r" | "ready" => local_action = Some(Action::Ready),
                "" => {}
                other => println!("unknown command: {other:?}  (h/s/r/q)"),
            },
            Event::Net(Msg::Action(a)) if is_host => {
                state.apply(1, a, &mut shoe);
                net::send(&out, Msg::State(state.clone()));
                changed = true;
            }
            Event::Net(Msg::State(s)) if !is_host => {
                state = s;
                changed = true;
            }
            Event::Net(_) => {} // wrong direction, ignore
            Event::Peer => {
                if is_host {
                    println!("player 2 connected");
                    net::send(&out, Msg::State(state.clone()));
                }
            }
            Event::Disconnected => {
                println!("\nother player disconnected, bye");
                return Ok(());
            }
        }

        if let Some(a) = local_action {
            if is_host {
                state.apply(0, a, &mut shoe);
                net::send(&out, Msg::State(state.clone()));
                changed = true;
            } else {
                net::send(&out, Msg::Action(a));
                // client waits for the host's State snapshot before redrawing
            }
        }

        if changed {
            print!("{}", render(&state, me));
            std::io::stdout().flush()?;
        }
    }
}
