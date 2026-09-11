mod game;
mod net;
mod player;

use game::{hand_value, Action, Card, GameState, Outcome, Phase, DEALER, MAX_PLAYERS};
use net::{Event, HandResult, Msg, Report, Row};
use player::Profile;
use std::error::Error;
use std::io::Write;
use std::sync::mpsc;

fn usage() -> ! {
    eprintln!("usage: blackjack host\n       blackjack join <CODE>\n       blackjack scores\n       blackjack name [NEW]\n       blackjack --version");
    std::process::exit(2);
}

fn cards(hand: &[Card]) -> String {
    hand.iter().map(Card::label).collect::<Vec<_>>().join(" ")
}

fn name(seat: usize) -> String {
    if seat == DEALER { "Dealer".into() } else { format!("Player {seat}") }
}

fn render(state: &GameState, me: usize) -> String {
    let mut out = format!("\n===== Round {} =====\n", state.round);
    let between_rounds = matches!(state.phase, Phase::WaitingForReady | Phase::RoundOver);
    for (seat, chair) in state.seated() {
        let marker = if state.to_act() == Some(seat) { ">" } else { " " };
        let you = if seat == me { " (you)" } else { "" };
        out += &format!("{marker} {:<14}: ", format!("{}{you}", name(seat)));
        // the dealer's second card stays face down until every player has finished
        let hole_hidden = seat == DEALER && me != DEALER && state.phase == Phase::PlayerTurn;
        if hole_hidden {
            out += &format!("{} ??", chair.hand.first().map(Card::label).unwrap_or_default());
        } else if chair.hand.is_empty() && !between_rounds {
            out += "joins next round";
        } else {
            out += &cards(&chair.hand);
            if !chair.hand.is_empty() {
                let v = hand_value(&chair.hand);
                out += &format!("  = {v}");
                if v > 21 {
                    out += "  BUST";
                }
            }
        }
        if let Some(r) = chair.result {
            out += match r {
                Outcome::Win => "  WIN",
                Outcome::Lose => "  LOSE",
                Outcome::Push => "  PUSH",
            };
        }
        if between_rounds {
            out += if chair.ready { "  [ready]" } else { "  [not ready]" };
        }
        if !chair.name.is_empty() {
            out += &format!("  {} {}", chair.name, chair.tally.label());
        }
        out.push('\n');
    }
    out += &match state.to_act() {
        Some(seat) if seat == me => "Your turn: (h)it, (s)tand, (q)uit".to_owned(),
        Some(DEALER) => "Waiting for the dealer... (q)uit".to_owned(),
        Some(seat) => format!("Waiting for player {seat}... (q)uit"),
        None => "(r)eady for next round, (q)uit".to_owned(),
    };
    out += "\n> ";
    out
}

/// The lifetime leaderboard as a table, the caller's own row marked.
fn leaderboard(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "no hands recorded yet\n".into();
    }
    let width = rows.iter().map(|r| r.name.chars().count()).max().unwrap_or(0).max(4);
    let mut out = format!(" {:>2}  {:<width$}  {:>4} {:>4} {:>4}  hands\n", "#", "name", "W", "L", "P");
    for (i, r) in rows.iter().enumerate() {
        let you = if r.you { "  (you)" } else { "" };
        out += &format!(" {:>2}  {:<width$}  {:>4} {:>4} {:>4}  {:>5}{you}\n", i + 1, r.name, r.wins, r.losses, r.pushes, r.hands);
    }
    out
}

/// `blackjack scores`: no profile is made just to look.
fn scores() -> Result<(), Box<dyn Error>> {
    let who = Profile::load()?.unwrap_or(Profile { id: String::new(), name: String::new() });
    print!("{}", leaderboard(&net::scores(&who)?));
    Ok(())
}

/// `blackjack name [NEW]`: shows or changes the name the scoreboard shows for this computer.
fn rename(new: Option<&str>) -> Result<(), Box<dyn Error>> {
    let path = player::path()?;
    let saved = Profile::load()?;
    match new {
        None => match saved {
            Some(p) => println!("you are {} ({})", p.name, path.display()),
            None => println!("no name yet; it is asked for on your first host or join, or run: blackjack name <NAME>"),
        },
        Some(new) => {
            let name = player::valid_name(new).ok_or_else(|| format!("a name is 1 to {} printable characters", player::MAX_NAME))?;
            let profile = match saved {
                Some(p) => Profile { name, ..p },
                None => Profile { id: player::new_id(), name },
            };
            profile.save();
            println!("you are now {} ({})", profile.name, path.display());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (role, code) = match args.first().map(String::as_str) {
        Some("host") => ("host", (0..4).map(|_| (b'A' + rand::random_range(0..26u8)) as char).collect::<String>()),
        Some("join") => ("join", args.get(1).unwrap_or_else(|| usage()).to_uppercase()),
        Some("scores") => return scores(),
        Some("name") => return rename(args.get(1).map(String::as_str)),
        Some("--version" | "-V" | "version") => {
            println!("blackjack {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => usage(),
    };
    let who = Profile::load_or_create()?; // may ask for a name; stdin is still ours here
    let (tx, rx) = mpsc::channel();
    let (out, me) = net::connect(&code, role, &who, tx.clone())?;
    let is_host = me == DEALER;
    if is_host {
        println!("room {code} open, waiting for players (up to {MAX_PLAYERS}) ...");
        println!("players run:  blackjack join {code}");
    } else {
        println!("joined room {code} as player {me}, waiting for the dealer ...");
    }
    net::spawn_input(tx);

    let mut state = GameState::new();
    let mut shoe = Vec::new();
    let mut reported = 0u32; // the last round whose results went to the scoreboard
    if is_host {
        state.set_name(DEALER, &who.name);
    }
    // Joiners render when the host's first state arrives.
    if is_host {
        print!("{}", render(&state, me));
        std::io::stdout().flush()?;
    }

    loop {
        let mut local_action = None;
        let mut changed = false;
        match rx.recv()? {
            Event::Line(line) => match line.trim() {
                "q" | "quit" => {
                    out.close(); // tells the relay, so the others hear now
                    return Ok(());
                }
                "h" | "hit" => local_action = Some(Action::Hit),
                "s" | "stand" => local_action = Some(Action::Stand),
                "r" | "ready" => local_action = Some(Action::Ready),
                "" => {}
                other => println!("unknown command: {other:?}  (h/s/r/q)"),
            },
            Event::Net(from, Msg::Action(a)) if is_host => {
                state.apply(from, a, &mut shoe);
                out.send(Msg::State(Box::new(state.clone())));
                changed = true;
            }
            Event::Net(_, Msg::State(s)) if !is_host => {
                state = *s;
                changed = true;
            }
            Event::Net(..) => {} // wrong direction, ignore
            Event::Joined(seat, name) if is_host => {
                state.sit(seat);
                let name = name.unwrap_or_default();
                state.set_name(seat, &name);
                if name.is_empty() {
                    println!("player {seat} joined");
                } else {
                    println!("player {seat} ({name}) joined");
                }
                out.send(Msg::State(Box::new(state.clone())));
                changed = true;
            }
            Event::Left(seat) if is_host => {
                state.leave(seat, &mut shoe);
                println!("player {seat} left");
                out.send(Msg::State(Box::new(state.clone())));
                changed = true;
            }
            Event::Joined(..) | Event::Left(_) => {} // the host tells us in the next snapshot
            Event::Disconnected(why) => {
                println!("\n{why}, bye");
                return Ok(());
            }
        }

        if let Some(a) = local_action {
            if is_host {
                state.apply(DEALER, a, &mut shoe);
                out.send(Msg::State(Box::new(state.clone())));
                changed = true;
            } else {
                out.send(Msg::Action(a));
                // players wait for the host's State snapshot before redrawing
            }
        }

        // A round can end on an action or on a player leaving; either way it is reported once,
        // after the snapshot that shows it and before any `leave` of our own.
        if is_host && state.phase == Phase::RoundOver && state.round > reported {
            reported = state.round;
            let results: Vec<HandResult> = state.players().filter_map(|(seat, s)| s.result.map(|outcome| HandResult { seat, outcome })).collect();
            if !results.is_empty() {
                out.report(Report { round: state.round, results });
            }
        }

        if changed {
            print!("{}", render(&state, me));
            std::io::stdout().flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::Seat;

    const P1: usize = 1;
    const P2: usize = 2;

    fn hand(ranks: &[u8], suit: u8) -> Vec<Card> {
        ranks.iter().map(|&rank| Card { rank, suit }).collect()
    }

    fn chair(ranks: &[u8], suit: u8) -> Option<Seat> {
        Some(Seat { hand: hand(ranks, suit), ..Default::default() })
    }

    /// Round 1, the dealer's cards in hearts and player 1's in spades, no result yet.
    fn table(player: &[u8], dealer: &[u8], phase: Phase) -> GameState {
        let mut g = GameState::new();
        g.seats[DEALER] = chair(dealer, 1);
        g.seats[P1] = chair(player, 0);
        g.phase = phase;
        g.turn = P1;
        g.round = 1;
        g
    }

    /// The table line for one seat.
    fn line_for(out: &str, name: &str) -> String {
        out.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("no {name} line in:{out}")).to_owned()
    }

    /// The instruction line under the table.
    fn prompt(state: &GameState, me: usize) -> String {
        render(state, me).lines().rev().nth(1).unwrap().to_owned()
    }

    fn set_result(g: &mut GameState, seat: usize, r: Outcome) {
        g.seats[seat].as_mut().unwrap().result = Some(r);
    }

    #[test]
    fn dealer_hand_visibility_and_prompts_follow_the_turn() {
        let mut state = table(&[10, 7], &[9, 5], Phase::PlayerTurn);

        let host = render(&state, DEALER);
        let joiner = render(&state, P1);
        assert!(host.contains("Dealer (you)  : 9♥ 5♥  = 14"), "{host}");
        assert!(host.contains("Waiting for player 1..."), "{host}");
        assert!(joiner.contains("9♥ ??"), "{joiner}");
        assert!(!joiner.contains("5♥"), "{joiner}");
        assert!(joiner.contains("> Player 1 (you)"), "{joiner}");
        assert!(joiner.contains("Your turn: (h)it, (s)tand"), "{joiner}");

        state.apply(P1, Action::Stand, &mut vec![]);
        let host = render(&state, DEALER);
        let joiner = render(&state, P1);
        assert!(host.contains("> Dealer (you)"), "{host}");
        assert!(host.contains("Your turn: (h)it, (s)tand"), "{host}");
        assert!(joiner.contains("9♥ 5♥  = 14"), "{joiner}");
        assert!(joiner.contains("Waiting for the dealer"), "{joiner}");

        state.apply(DEALER, Action::Stand, &mut vec![]);
        assert!(render(&state, DEALER).contains("  Player 1      : 10♠ 7♠  = 17  WIN"));
        assert!(render(&state, P1).contains("  Player 1 (you): 10♠ 7♠  = 17  WIN"));
        assert!(render(&state, P1).contains("  Dealer        : 9♥ 5♥  = 14  [not ready]"), "no single result on the dealer's line");
    }

    #[test]
    fn a_fresh_table_is_the_dealer_alone_with_a_ready_flag() {
        assert_eq!(
            render(&GameState::new(), DEALER),
            "\n===== Round 0 =====\n  Dealer (you)  :   [not ready]\n(r)eady for next round, (q)uit\n> "
        );
        let mut g = GameState::new();
        g.sit(P1);
        assert_eq!(
            render(&g, P1),
            "\n===== Round 0 =====\n  Dealer        :   [not ready]\n  Player 1 (you):   [not ready]\n(r)eady for next round, (q)uit\n> "
        );
    }

    #[test]
    fn every_seated_player_gets_a_line_in_seat_order_and_empty_chairs_none() {
        let mut g = GameState::new();
        for seat in [9, 4, 2] {
            g.sit(seat);
        }
        let out = render(&g, 4);
        let names: Vec<&str> = out.lines().filter(|l| l.contains(':')).map(|l| l[2..16].trim_end()).collect();
        assert_eq!(names, ["Dealer", "Player 2", "Player 4 (you)", "Player 9"]);
        assert!(!out.contains("Player 1"), "{out}");
    }

    #[test]
    fn the_round_number_heads_the_table() {
        let mut g = GameState::new();
        g.round = 12;
        assert!(render(&g, DEALER).starts_with("\n===== Round 12 =====\n"));
    }

    #[test]
    fn ready_flags_follow_each_seat_between_rounds() {
        let mut g = GameState::new();
        g.sit(P1);
        g.sit(P2);
        g.apply(DEALER, Action::Ready, &mut vec![]);
        g.apply(P2, Action::Ready, &mut vec![]);
        let out = render(&g, DEALER);
        assert!(line_for(&out, "Dealer").ends_with("[ready]"), "{out}");
        assert!(line_for(&out, "Player 1").ends_with("[not ready]"), "{out}");
        assert!(line_for(&out, "Player 2").ends_with("[ready]"), "{out}");
    }

    #[test]
    fn ready_flags_are_hidden_mid_round() {
        for phase in [Phase::PlayerTurn, Phase::DealerTurn] {
            assert!(!render(&table(&[10, 7], &[9, 5], phase), DEALER).contains("ready]"), "{phase:?}");
        }
    }

    #[test]
    fn a_bust_is_called_out_next_to_the_total() {
        let mut g = table(&[10, 9, 5], &[10, 7], Phase::RoundOver);
        set_result(&mut g, P1, Outcome::Lose);
        let out = render(&g, P1);
        assert_eq!(line_for(&out, "Player 1"), "  Player 1 (you): 10♠ 9♠ 5♠  = 24  BUST  LOSE  [not ready]");
        assert_eq!(line_for(&out, "Dealer"), "  Dealer        : 10♥ 7♥  = 17  [not ready]");
    }

    #[test]
    fn each_player_sees_their_own_result_and_everyone_elses() {
        let mut g = table(&[10, 8], &[9, 9], Phase::RoundOver);
        g.seats[P2] = chair(&[10, 9], 2);
        g.seats[5] = chair(&[10, 5, 9], 3);
        set_result(&mut g, P1, Outcome::Push);
        set_result(&mut g, P2, Outcome::Win);
        set_result(&mut g, 5, Outcome::Lose);
        for me in [DEALER, P1, P2, 5] {
            let out = render(&g, me);
            assert!(line_for(&out, "Dealer").contains("= 18  [not ready]"), "{out}");
            assert!(line_for(&out, "Player 1").contains("= 18  PUSH"), "{out}");
            assert!(line_for(&out, "Player 2").contains("= 19  WIN"), "{out}");
            assert!(line_for(&out, "Player 5").contains("= 24  BUST  LOSE"), "{out}");
        }
    }

    #[test]
    fn the_hole_card_is_hidden_from_every_player_only_while_players_decide() {
        let mut g = table(&[10, 7], &[9, 5], Phase::PlayerTurn);
        g.seats[P2] = chair(&[8, 8], 2);
        assert_eq!(line_for(&render(&g, P1), "Dealer"), "  Dealer        : 9♥ ??");
        assert_eq!(line_for(&render(&g, P2), "Dealer"), "  Dealer        : 9♥ ??", "player 2 waits their turn and still cannot see it");
        assert_eq!(line_for(&render(&g, DEALER), "Dealer"), "  Dealer (you)  : 9♥ 5♥  = 14");
        for phase in [Phase::DealerTurn, Phase::RoundOver, Phase::WaitingForReady] {
            g.phase = phase;
            for me in [P1, P2] {
                assert!(line_for(&render(&g, me), "Dealer").contains("9♥ 5♥  = 14"), "{phase:?}");
            }
        }
    }

    #[test]
    fn the_marker_points_at_whoever_is_to_act() {
        let mut g = table(&[10, 7], &[9, 5], Phase::PlayerTurn);
        g.seats[P2] = chair(&[8, 8], 2);
        let out = render(&g, DEALER);
        assert!(line_for(&out, "Player 1").starts_with("> Player 1"));
        assert!(line_for(&out, "Player 2").starts_with("  Player 2"));
        assert!(line_for(&out, "Dealer").starts_with("  Dealer"));
        g.turn = P2;
        let out = render(&g, DEALER);
        assert!(line_for(&out, "Player 1").starts_with("  Player 1"));
        assert!(line_for(&out, "Player 2").starts_with("> Player 2"));
        g.phase = Phase::DealerTurn;
        let out = render(&g, DEALER);
        assert!(line_for(&out, "Dealer").starts_with("> Dealer"));
        assert!(line_for(&out, "Player 1").starts_with("  Player 1"));
        assert!(line_for(&out, "Player 2").starts_with("  Player 2"));
        for phase in [Phase::RoundOver, Phase::WaitingForReady] {
            g.phase = phase;
            let out = render(&g, DEALER);
            assert!(out.lines().filter(|l| l.contains(':')).all(|l| l.starts_with("  ")), "{phase:?}: nobody is to act\n{out}");
        }
    }

    #[test]
    fn prompts_match_the_seat_and_the_phase() {
        let at = |phase| {
            let mut g = table(&[10, 7], &[9, 5], phase);
            g.seats[P2] = chair(&[8, 8], 2);
            g
        };
        assert_eq!(prompt(&at(Phase::PlayerTurn), P1), "Your turn: (h)it, (s)tand, (q)uit");
        assert_eq!(prompt(&at(Phase::PlayerTurn), P2), "Waiting for player 1... (q)uit");
        assert_eq!(prompt(&at(Phase::PlayerTurn), DEALER), "Waiting for player 1... (q)uit");
        let mut g = at(Phase::PlayerTurn);
        g.turn = P2;
        assert_eq!(prompt(&g, P1), "Waiting for player 2... (q)uit");
        assert_eq!(prompt(&g, P2), "Your turn: (h)it, (s)tand, (q)uit");
        assert_eq!(prompt(&at(Phase::DealerTurn), DEALER), "Your turn: (h)it, (s)tand, (q)uit");
        assert_eq!(prompt(&at(Phase::DealerTurn), P1), "Waiting for the dealer... (q)uit");
        assert_eq!(prompt(&at(Phase::DealerTurn), P2), "Waiting for the dealer... (q)uit");
        for phase in [Phase::WaitingForReady, Phase::RoundOver] {
            for me in [DEALER, P1, P2] {
                assert_eq!(prompt(&at(phase), me), "(r)eady for next round, (q)uit");
            }
        }
    }

    #[test]
    fn a_player_who_joined_mid_round_is_shown_sitting_out() {
        let mut g = table(&[10, 7], &[9, 5], Phase::PlayerTurn);
        g.sit(P2);
        assert_eq!(line_for(&render(&g, P2), "Player 2"), "  Player 2 (you): joins next round");
        assert_eq!(prompt(&g, P2), "Waiting for player 1... (q)uit");
        g.phase = Phase::RoundOver;
        set_result(&mut g, P1, Outcome::Win);
        assert_eq!(line_for(&render(&g, P2), "Player 2"), "  Player 2 (you):   [not ready]", "nothing to show for a hand never dealt");
    }

    #[test]
    fn a_player_not_yet_seated_in_the_snapshot_still_sees_the_table() {
        // the host's snapshot can arrive before the host has processed our "joined"
        let g = table(&[10, 7], &[9, 5], Phase::PlayerTurn);
        let out = render(&g, 6);
        assert!(!out.contains("(you)"), "{out}");
        assert_eq!(prompt(&g, 6), "Waiting for player 1... (q)uit");
    }

    #[test]
    fn the_table_ends_with_an_input_prompt() {
        assert!(render(&table(&[10, 7], &[9, 5], Phase::PlayerTurn), P1).ends_with("\n> "));
    }

    #[test]
    fn cards_are_labels_separated_by_spaces() {
        assert_eq!(cards(&[]), "");
        assert_eq!(cards(&[Card { rank: 10, suit: 0 }, Card { rank: 1, suit: 1 }]), "10♠ A♥");
    }

    #[test]
    fn seats_are_named_for_people() {
        assert_eq!(name(DEALER), "Dealer");
        assert_eq!(name(1), "Player 1");
        assert_eq!(name(MAX_PLAYERS), "Player 9");
    }

    // ---- names and scores ---------------------------------------------------------------

    #[test]
    fn a_named_seat_ends_with_who_sits_there_and_their_tally() {
        let mut g = table(&[10, 9, 5], &[10, 7], Phase::RoundOver);
        g.set_name(DEALER, "alice");
        g.set_name(P1, "bob");
        set_result(&mut g, P1, Outcome::Lose);
        g.seats[P1].as_mut().unwrap().tally.record(Outcome::Lose);
        g.seats[DEALER].as_mut().unwrap().tally = game::Tally { wins: 3, losses: 1, pushes: 2 };
        let out = render(&g, P1);
        assert_eq!(line_for(&out, "Player 1"), "  Player 1 (you): 10♠ 9♠ 5♠  = 24  BUST  LOSE  [not ready]  bob 0W 1L 0P");
        assert_eq!(line_for(&out, "Dealer"), "  Dealer        : 10♥ 7♥  = 17  [not ready]  alice 3W 1L 2P");
        g.phase = Phase::PlayerTurn;
        assert_eq!(line_for(&render(&g, DEALER), "Player 1"), "> Player 1      : 10♠ 9♠ 5♠  = 24  BUST  LOSE  bob 0W 1L 0P", "shown mid-round too");
    }

    #[test]
    fn an_unnamed_seat_shows_no_tally() {
        let mut g = table(&[10, 7], &[9, 5], Phase::RoundOver);
        g.seats[P1].as_mut().unwrap().tally.record(Outcome::Win);
        assert_eq!(line_for(&render(&g, P1), "Player 1"), "  Player 1 (you): 10♠ 7♠  = 17  [not ready]");
    }

    fn row(name: &str, w: u32, l: u32, p: u32, you: bool) -> Row {
        Row { name: name.into(), wins: w, losses: l, pushes: p, hands: w + l + p, you }
    }

    #[test]
    fn the_leaderboard_is_a_ranked_table_with_our_row_marked() {
        let rows = [row("alice", 12, 8, 2, false), row("bob", 8, 12, 2, true), row("a-longer-name", 0, 0, 0, false)];
        assert_eq!(
            leaderboard(&rows),
            concat!(
                "  #  name              W    L    P  hands\n",
                "  1  alice            12    8    2     22\n",
                "  2  bob               8   12    2     22  (you)\n",
                "  3  a-longer-name     0    0    0      0\n",
            )
        );
        assert_eq!(leaderboard(&[]), "no hands recorded yet\n");
    }
}
