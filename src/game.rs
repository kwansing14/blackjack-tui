use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub rank: u8, // 1 = Ace .. 13 = King
    pub suit: u8, // 0..4
}

impl Card {
    pub fn label(&self) -> String {
        let r = match self.rank {
            1 => "A".into(),
            11 => "J".into(),
            12 => "Q".into(),
            13 => "K".into(),
            n => n.to_string(),
        };
        let s = ['♠', '♥', '♦', '♣'][self.suit as usize % 4];
        format!("{r}{s}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Hit,
    Stand,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    WaitingForReady,
    PlayerTurn(usize),
    RoundOver,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Win,
    Lose,
    Push,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    pub players: [Vec<Card>; 2],
    pub dealer: Vec<Card>,
    pub phase: Phase,
    pub ready: [bool; 2],
    pub results: Option<[Outcome; 2]>,
    pub round: u32,
}

pub fn hand_value(cards: &[Card]) -> u8 {
    let mut total: u8 = 0;
    let mut aces = 0;
    for c in cards {
        total += match c.rank {
            1 => {
                aces += 1;
                11
            }
            11..=13 => 10,
            n => n,
        };
    }
    while total > 21 && aces > 0 {
        total -= 10;
        aces -= 1;
    }
    total
}

fn fresh_shoe() -> Vec<Card> {
    let mut shoe: Vec<Card> = (0..4)
        .flat_map(|suit| (1..=13).map(move |rank| Card { rank, suit }))
        .collect();
    shoe.shuffle(&mut rand::rng());
    shoe
}

impl GameState {
    pub fn new() -> Self {
        Self {
            players: [vec![], vec![]],
            dealer: vec![],
            phase: Phase::WaitingForReady,
            ready: [false, false],
            results: None,
            round: 0,
        }
    }

    /// Host-only. `shoe` is drawn from and reshuffled at the start of each round.
    /// Out-of-turn or out-of-phase actions are ignored.
    pub fn apply(&mut self, player: usize, action: Action, shoe: &mut Vec<Card>) {
        match (self.phase, action) {
            (Phase::WaitingForReady | Phase::RoundOver, Action::Ready) => {
                self.ready[player] = true;
                if self.ready == [true, true] {
                    self.start_round(shoe);
                }
            }
            (Phase::PlayerTurn(p), Action::Hit) if p == player => {
                self.players[p].push(shoe.pop().expect("shoe never empties in one round"));
                if hand_value(&self.players[p]) >= 21 {
                    self.advance(shoe);
                }
            }
            (Phase::PlayerTurn(p), Action::Stand) if p == player => self.advance(shoe),
            _ => {}
        }
    }

    fn start_round(&mut self, shoe: &mut Vec<Card>) {
        *shoe = fresh_shoe(); // ponytail: reshuffle every round, no card counting to worry about
        self.round += 1;
        self.ready = [false, false];
        self.results = None;
        for hand in self.players.iter_mut() {
            *hand = vec![shoe.pop().unwrap(), shoe.pop().unwrap()];
        }
        self.dealer = vec![shoe.pop().unwrap(), shoe.pop().unwrap()];
        self.phase = Phase::PlayerTurn(0);
        // skip players dealt a natural 21
        if hand_value(&self.players[0]) == 21 {
            self.advance(shoe);
        }
    }

    fn advance(&mut self, shoe: &mut Vec<Card>) {
        match self.phase {
            Phase::PlayerTurn(0) => {
                self.phase = Phase::PlayerTurn(1);
                if hand_value(&self.players[1]) == 21 {
                    self.advance(shoe);
                }
            }
            Phase::PlayerTurn(_) => self.dealer_play(shoe),
            _ => {}
        }
    }

    fn dealer_play(&mut self, shoe: &mut Vec<Card>) {
        let anyone_alive = self.players.iter().any(|h| hand_value(h) <= 21);
        if anyone_alive {
            while hand_value(&self.dealer) < 17 {
                self.dealer.push(shoe.pop().unwrap());
            }
        }
        let d = hand_value(&self.dealer);
        let outcome = |hand: &[Card]| {
            let p = hand_value(hand);
            if p > 21 {
                Outcome::Lose
            } else if d > 21 || p > d {
                Outcome::Win
            } else if p == d {
                Outcome::Push
            } else {
                Outcome::Lose
            }
        };
        self.results = Some([outcome(&self.players[0]), outcome(&self.players[1])]);
        self.phase = Phase::RoundOver;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(rank: u8) -> Card {
        Card { rank, suit: 0 }
    }

    #[test]
    fn values() {
        assert_eq!(hand_value(&[c(1), c(13)]), 21);
        assert_eq!(hand_value(&[c(1), c(1)]), 12);
        assert_eq!(hand_value(&[c(1), c(9), c(1)]), 21);
        assert_eq!(hand_value(&[c(13), c(12), c(2)]), 22);
        assert_eq!(hand_value(&[c(1), c(1), c(1), c(1)]), 14);
    }

    #[test]
    fn rejects_out_of_turn_and_plays_a_round() {
        let mut g = GameState::new();
        let mut shoe = vec![];
        g.apply(0, Action::Hit, &mut shoe); // ignored, not started
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.apply(0, Action::Ready, &mut shoe);
        g.apply(1, Action::Ready, &mut shoe);
        assert_eq!(g.round, 1);
        assert_eq!(shoe.len(), 52 - 6);
        if g.phase == Phase::PlayerTurn(0) {
            let before = g.players[0].len();
            g.apply(1, Action::Hit, &mut shoe); // wrong player, ignored
            assert_eq!(g.players[0].len(), before);
            g.apply(0, Action::Stand, &mut shoe);
        }
        if let Phase::PlayerTurn(1) = g.phase {
            g.apply(1, Action::Stand, &mut shoe);
        }
        assert_eq!(g.phase, Phase::RoundOver);
        assert!(g.results.is_some());
        assert!(hand_value(&g.dealer) >= 17);
    }
}
