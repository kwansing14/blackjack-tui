mod game;
mod net;

use game::{hand_value, Action, Card, GameState, Outcome, Phase, DEALER, PLAYER};
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dealer_hand_visibility_and_prompts_follow_the_turn() {
        let mut state = GameState::new();
        state.player = vec![Card { rank: 10, suit: 0 }, Card { rank: 7, suit: 0 }];
        state.dealer = vec![Card { rank: 9, suit: 1 }, Card { rank: 5, suit: 1 }];
        state.phase = Phase::PlayerTurn;

        let host = render(&state, DEALER);
        let joiner = render(&state, PLAYER);
        assert!(host.contains("Dealer (you): 9♥ 5♥  = 14"));
        assert!(host.contains("Waiting for the player"));
        assert!(joiner.contains("9♥ ??"));
        assert!(!joiner.contains("5♥"));
        assert!(joiner.contains("> Player (you)"));
        assert!(joiner.contains("Your turn: (h)it, (s)tand"));

        state.apply(PLAYER, Action::Stand, &mut vec![]);
        let host = render(&state, DEALER);
        let joiner = render(&state, PLAYER);
        assert!(host.contains("> Dealer (you)"));
        assert!(host.contains("Your turn: (h)it, (s)tand"));
        assert!(joiner.contains("9♥ 5♥  = 14"));
        assert!(joiner.contains("Waiting for the dealer"));

        state.apply(DEALER, Action::Stand, &mut vec![]);
        assert!(render(&state, DEALER).contains("Dealer (you): 9♥ 5♥  = 14  LOSE"));
        assert!(render(&state, PLAYER).contains("Player (you): 10♠ 7♠  = 17  WIN"));
    }
}

fn render(state: &GameState, me: usize) -> String {
    let mut out = format!("\n===== Round {} =====\n", state.round);
    let between_rounds = matches!(state.phase, Phase::WaitingForReady | Phase::RoundOver);
    for seat in [DEALER, PLAYER] {
        let (name, hand) = if seat == DEALER {
            ("Dealer", &state.dealer)
        } else {
            ("Player", &state.player)
        };
        let their_turn = match state.phase {
            Phase::DealerTurn => seat == DEALER,
            Phase::PlayerTurn => seat == PLAYER,
            _ => false,
        };
        let marker = if their_turn { ">" } else { " " };
        let you = if seat == me { " (you)" } else { "      " };
        out += &format!("{marker} {name}{you}: ");
        // the dealer's second card stays face down until the player has finished
        let hole_hidden = seat == DEALER && me != DEALER && state.phase == Phase::PlayerTurn;
        if hole_hidden {
            out += &format!("{} ??", hand[0].label());
        } else {
            out += &cards(hand);
            if !hand.is_empty() {
                let v = hand_value(hand);
                out += &format!("  = {v}");
                if v > 21 {
                    out += "  BUST";
                }
            }
        }
        if let Some(r) = state.result {
            let r = if seat == PLAYER { r } else { r.opposite() };
            out += match r {
                Outcome::Win => "  WIN",
                Outcome::Lose => "  LOSE",
                Outcome::Push => "  PUSH",
            };
        }
        if between_rounds {
            out += if state.ready[seat] {
                "  [ready]"
            } else {
                "  [not ready]"
            };
        }
        out.push('\n');
    }
    out += match state.phase {
        Phase::PlayerTurn if me == PLAYER => "Your turn: (h)it, (s)tand, (q)uit",
        Phase::DealerTurn if me == DEALER => "Your turn: (h)it, (s)tand, (q)uit",
        Phase::PlayerTurn => "Waiting for the player... (q)uit",
        Phase::DealerTurn => "Waiting for the dealer... (q)uit",
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
            (out, DEALER)
        }
        Some("join") => {
            let code = args.get(1).unwrap_or_else(|| usage()).to_uppercase();
            (net::connect(&code, "join", tx.clone())?, PLAYER)
        }
        _ => usage(),
    };
    let is_host = me == DEALER;
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
                    out.close(); // tells the relay, so the other player hears now
                    return Ok(());
                }
                "h" | "hit" => local_action = Some(Action::Hit),
                "s" | "stand" => local_action = Some(Action::Stand),
                "r" | "ready" => local_action = Some(Action::Ready),
                "" => {}
                other => println!("unknown command: {other:?}  (h/s/r/q)"),
            },
            Event::Net(Msg::Action(a)) if is_host => {
                state.apply(PLAYER, a, &mut shoe);
                out.send(Msg::State(state.clone()));
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
                    out.send(Msg::State(state.clone()));
                }
            }
            Event::Disconnected(why) => {
                println!("\n{why}, bye");
                return Ok(());
            }
        }

        if let Some(a) = local_action {
            if is_host {
                state.apply(DEALER, a, &mut shoe);
                out.send(Msg::State(state.clone()));
                changed = true;
            } else {
                out.send(Msg::Action(a));
                // client waits for the host's State snapshot before redrawing
            }
        }

        if changed {
            print!("{}", render(&state, me));
            std::io::stdout().flush()?;
        }
    }
}
