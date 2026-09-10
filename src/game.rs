use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

/// Seats. The host deals and plays the house hand; the joiner plays against it.
pub const DEALER: usize = 0;
pub const PLAYER: usize = 1;

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
    PlayerTurn,
    DealerTurn,
    RoundOver,
}

/// The round's result as the player sees it; the dealer's is the opposite.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Win,
    Lose,
    Push,
}

impl Outcome {
    pub fn opposite(self) -> Self {
        match self {
            Outcome::Win => Outcome::Lose,
            Outcome::Lose => Outcome::Win,
            Outcome::Push => Outcome::Push,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    pub player: Vec<Card>,
    pub dealer: Vec<Card>,
    pub phase: Phase,
    pub ready: [bool; 2], // by seat
    pub result: Option<Outcome>,
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

fn draw(shoe: &mut Vec<Card>) -> Card {
    shoe.pop().expect("a 52-card shoe outlasts one round")
}

impl GameState {
    pub fn new() -> Self {
        Self {
            player: vec![],
            dealer: vec![],
            phase: Phase::WaitingForReady,
            ready: [false, false],
            result: None,
            round: 0,
        }
    }

    /// Host-only. `shoe` is drawn from and reshuffled at the start of each round.
    /// Out-of-turn or out-of-phase actions are ignored.
    pub fn apply(&mut self, seat: usize, action: Action, shoe: &mut Vec<Card>) {
        match (self.phase, action) {
            (Phase::WaitingForReady | Phase::RoundOver, Action::Ready) => {
                self.ready[seat] = true;
                if self.ready == [true, true] {
                    self.start_round(shoe);
                }
            }
            (Phase::PlayerTurn, Action::Hit) if seat == PLAYER => {
                self.player.push(draw(shoe));
                if hand_value(&self.player) >= 21 {
                    self.end_player_turn();
                }
            }
            (Phase::PlayerTurn, Action::Stand) if seat == PLAYER => self.end_player_turn(),
            (Phase::DealerTurn, Action::Hit) if seat == DEALER => {
                self.dealer.push(draw(shoe));
                if hand_value(&self.dealer) >= 21 {
                    self.settle();
                }
            }
            (Phase::DealerTurn, Action::Stand) if seat == DEALER => self.settle(),
            _ => {}
        }
    }

    fn start_round(&mut self, shoe: &mut Vec<Card>) {
        *shoe = fresh_shoe(); // ponytail: reshuffle every round, no card counting to worry about
        self.round += 1;
        self.ready = [false, false];
        self.result = None;
        self.player = vec![draw(shoe), draw(shoe)];
        self.dealer = vec![draw(shoe), draw(shoe)];
        self.phase = Phase::PlayerTurn;
        if hand_value(&self.player) == 21 {
            self.end_player_turn(); // a natural leaves nothing to decide
        }
    }

    /// The dealer only plays against a live hand, and a dealer already on 21 has nothing to decide.
    fn end_player_turn(&mut self) {
        if hand_value(&self.player) > 21 || hand_value(&self.dealer) == 21 {
            self.settle();
        } else {
            self.phase = Phase::DealerTurn;
        }
    }

    fn settle(&mut self) {
        let p = hand_value(&self.player);
        let d = hand_value(&self.dealer);
        self.result = Some(if p > 21 {
            Outcome::Lose
        } else if d > 21 || p > d {
            Outcome::Win
        } else if p == d {
            Outcome::Push
        } else {
            Outcome::Lose
        });
        self.phase = Phase::RoundOver;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(rank: u8) -> Card {
        Card { rank, suit: 0 }
    }

    /// A round already dealt with known cards; tests draw from the end of their own shoe.
    fn dealt(player: &[u8], dealer: &[u8]) -> GameState {
        GameState {
            player: player.iter().copied().map(c).collect(),
            dealer: dealer.iter().copied().map(c).collect(),
            phase: Phase::PlayerTurn,
            ready: [false, false],
            result: None,
            round: 1,
        }
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
    fn both_ready_deals_two_each_and_the_player_goes_first() {
        let mut g = GameState::new();
        let mut shoe = vec![];
        g.apply(PLAYER, Action::Hit, &mut shoe); // ignored, not started
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.apply(DEALER, Action::Ready, &mut shoe);
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.apply(PLAYER, Action::Ready, &mut shoe);
        assert_eq!(g.round, 1);
        assert_eq!(shoe.len(), 52 - 4);
        assert_eq!((g.player.len(), g.dealer.len()), (2, 2));
        assert_eq!(g.ready, [false, false]);
        if hand_value(&g.player) != 21 {
            assert_eq!(g.phase, Phase::PlayerTurn);
        }
    }

    #[test]
    fn dealer_plays_after_the_player_stands() {
        let mut g = dealt(&[10, 7], &[9, 5]);
        let mut shoe = vec![c(5), c(2)]; // drawn from the end: 2, then 5
        g.apply(DEALER, Action::Hit, &mut shoe); // not the dealer's turn yet
        assert_eq!(g.dealer.len(), 2);
        g.apply(PLAYER, Action::Stand, &mut shoe);
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(PLAYER, Action::Hit, &mut shoe); // player is done, ignored
        assert_eq!(g.player.len(), 2);
        g.apply(DEALER, Action::Hit, &mut shoe); // 14 + 2 = 16
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(DEALER, Action::Hit, &mut shoe); // 16 + 5 = 21, nothing left to decide
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(g.result, Some(Outcome::Lose)); // 17 against 21
        assert_eq!(g.result.unwrap().opposite(), Outcome::Win);
    }

    #[test]
    fn dealer_may_stand_short_and_lose() {
        let mut g = dealt(&[10, 9], &[10, 7]);
        let mut shoe = vec![c(2)];
        g.apply(PLAYER, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Stand, &mut shoe);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(g.result, Some(Outcome::Win));
        assert_eq!(shoe.len(), 1);
    }

    #[test]
    fn equal_totals_push() {
        let mut g = dealt(&[10, 8], &[10, 8]);
        g.apply(PLAYER, Action::Stand, &mut vec![]);
        g.apply(DEALER, Action::Stand, &mut vec![]);
        assert_eq!(g.result, Some(Outcome::Push));
        assert_eq!(Outcome::Push.opposite(), Outcome::Push);
    }

    #[test]
    fn player_bust_ends_the_round_without_the_dealer_playing() {
        let mut g = dealt(&[10, 6], &[2, 3]);
        let mut shoe = vec![c(10)];
        g.apply(PLAYER, Action::Hit, &mut shoe); // 26
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(g.result, Some(Outcome::Lose));
        assert_eq!(g.dealer.len(), 2);
    }

    #[test]
    fn dealer_bust_is_a_player_win() {
        let mut g = dealt(&[10, 2], &[10, 6]);
        let mut shoe = vec![c(10)];
        g.apply(PLAYER, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Hit, &mut shoe); // 26
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(g.result, Some(Outcome::Win));
    }

    #[test]
    fn dealer_natural_needs_no_decision() {
        let mut g = dealt(&[10, 7], &[1, 13]);
        g.apply(PLAYER, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(g.result, Some(Outcome::Lose));
    }

    #[test]
    fn ready_is_ignored_mid_round() {
        let mut g = dealt(&[10, 7], &[9, 5]);
        g.apply(DEALER, Action::Ready, &mut vec![]);
        assert_eq!(g.ready, [false, false]);
        assert_eq!(g.phase, Phase::PlayerTurn);
    }
}
