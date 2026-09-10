use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

/// Seat numbers. The host deals and plays the house hand from seat 0; up to nine joiners
/// take seats 1 to 9 and each play their own hand against it.
pub const DEALER: usize = 0;
pub const MAX_PLAYERS: usize = 9;
pub const SEATS: usize = MAX_PLAYERS + 1;

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

/// A player's result against the dealer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Win,
    Lose,
    Push,
}

impl Outcome {
    /// The same hand seen from the dealer's side.
    pub fn opposite(self) -> Self {
        match self {
            Outcome::Win => Outcome::Lose,
            Outcome::Lose => Outcome::Win,
            Outcome::Push => Outcome::Push,
        }
    }
}

/// Hands won, lost and pushed since this person sat down. The dealer's counts every hand
/// played against them, so it grows faster than any one player's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tally {
    pub wins: u32,
    pub losses: u32,
    pub pushes: u32,
}

impl Tally {
    pub fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Win => self.wins += 1,
            Outcome::Lose => self.losses += 1,
            Outcome::Push => self.pushes += 1,
        }
    }

    /// `2W 0L 1P`
    pub fn label(&self) -> String {
        format!("{}W {}L {}P", self.wins, self.losses, self.pushes)
    }
}

/// One chair at the table. Names and tallies belong to whoever sits there and go with them
/// when they leave; a newcomer starts from nothing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seat {
    pub hand: Vec<Card>,
    pub ready: bool,
    pub result: Option<Outcome>, // players only, once the round is settled
    #[serde(default)]
    pub name: String, // empty until the relay tells the host who sat down
    #[serde(default)]
    pub tally: Tally,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameState {
    /// Indexed by seat number; an empty chair is None. The dealer's chair is never empty.
    pub seats: [Option<Seat>; SEATS],
    pub phase: Phase,
    /// Whose turn it is while a round is being played; see `to_act`.
    pub turn: usize,
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

/// Ten hands can drink a single deck dry, so an empty shoe opens another one.
fn draw(shoe: &mut Vec<Card>) -> Card {
    if shoe.is_empty() {
        *shoe = fresh_shoe();
    }
    shoe.pop().expect("a fresh shoe is never empty")
}

/// Dealt in and not bust: the dealer still has this hand to beat.
fn live(seat: &Seat) -> bool {
    !seat.hand.is_empty() && hand_value(&seat.hand) <= 21
}

impl GameState {
    pub fn new() -> Self {
        let mut seats: [Option<Seat>; SEATS] = Default::default();
        seats[DEALER] = Some(Seat::default());
        Self { seats, phase: Phase::WaitingForReady, turn: DEALER, round: 0 }
    }

    /// Every occupied chair in seat order, the dealer first.
    pub fn seated(&self) -> impl Iterator<Item = (usize, &Seat)> {
        self.seats.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|s| (i, s)))
    }

    /// The players at the table, in seat order.
    pub fn players(&self) -> impl Iterator<Item = (usize, &Seat)> {
        self.seated().filter(|(i, _)| *i != DEALER)
    }

    pub fn dealer(&self) -> &Seat {
        self.seats[DEALER].as_ref().expect("the dealer never leaves")
    }

    /// The seat whose decision the table is waiting on, if any.
    pub fn to_act(&self) -> Option<usize> {
        match self.phase {
            Phase::PlayerTurn => Some(self.turn),
            Phase::DealerTurn => Some(DEALER),
            _ => None,
        }
    }

    fn occupied(&self, seat: usize) -> bool {
        matches!(self.seats.get(seat), Some(Some(_)))
    }

    fn seat_mut(&mut self, seat: usize) -> &mut Seat {
        self.seats[seat].as_mut().expect("checked with occupied()")
    }

    /// Host-only. `shoe` is drawn from and reshuffled at the start of each round.
    /// Actions from an empty chair, out of turn, or out of phase are ignored.
    pub fn apply(&mut self, seat: usize, action: Action, shoe: &mut Vec<Card>) {
        if !self.occupied(seat) {
            return;
        }
        match (self.phase, action) {
            (Phase::WaitingForReady | Phase::RoundOver, Action::Ready) => {
                self.seat_mut(seat).ready = true;
                self.maybe_start(shoe);
            }
            (Phase::PlayerTurn, Action::Hit) if seat == self.turn => {
                let hand = &mut self.seat_mut(seat).hand;
                hand.push(draw(shoe));
                if hand_value(hand) >= 21 {
                    self.next_turn();
                }
            }
            (Phase::PlayerTurn, Action::Stand) if seat == self.turn => self.next_turn(),
            (Phase::DealerTurn, Action::Hit) if seat == DEALER => {
                let hand = &mut self.seat_mut(DEALER).hand;
                hand.push(draw(shoe));
                if hand_value(hand) >= 21 {
                    self.settle();
                }
            }
            (Phase::DealerTurn, Action::Stand) if seat == DEALER => self.settle(),
            _ => {}
        }
    }

    /// Host-only: a player took `seat`. Mid-round they sit out until the next deal.
    /// The dealer's chair, a taken chair, and a seat number off the table are left alone.
    pub fn sit(&mut self, seat: usize) {
        if let Some(chair) = self.seats.get_mut(seat) {
            if chair.is_none() {
                *chair = Some(Seat::default());
            }
        }
    }

    /// Host-only: who is in `seat`. An empty chair keeps no name.
    pub fn set_name(&mut self, seat: usize, name: &str) {
        if let Some(Some(chair)) = self.seats.get_mut(seat) {
            chair.name = name.to_owned();
        }
    }

    /// Host-only: the player in `seat` left. Their turn passes to the next player, and a round
    /// that everyone else was waiting on them for starts without them.
    pub fn leave(&mut self, seat: usize, shoe: &mut Vec<Card>) {
        if seat == DEALER || !self.occupied(seat) {
            return;
        }
        self.seats[seat] = None;
        match self.phase {
            Phase::PlayerTurn if self.turn == seat => self.next_turn(),
            Phase::DealerTurn if !self.players().any(|(_, p)| live(p)) => self.settle(),
            Phase::WaitingForReady | Phase::RoundOver => self.maybe_start(shoe),
            _ => {}
        }
    }

    /// The deal waits for every seat, and the dealer alone has nobody to deal to.
    fn maybe_start(&mut self, shoe: &mut Vec<Card>) {
        if self.players().next().is_some() && self.seated().all(|(_, s)| s.ready) {
            self.start_round(shoe);
        }
    }

    fn start_round(&mut self, shoe: &mut Vec<Card>) {
        *shoe = fresh_shoe(); // ponytail: reshuffle every round, no card counting to worry about
        self.round += 1;
        for s in self.seats.iter_mut().flatten() {
            s.hand = vec![draw(shoe), draw(shoe)]; // name and tally stay with the person
            s.ready = false;
            s.result = None;
        }
        self.phase = Phase::PlayerTurn;
        self.turn = DEALER;
        self.next_turn(); // the first player with something to decide; a natural leaves nothing to decide
    }

    /// Passes the turn to the next player who still has a decision, or to the dealer if none is left.
    fn next_turn(&mut self) {
        let next = (self.turn + 1..SEATS)
            .find(|&i| self.seats[i].as_ref().is_some_and(|s| !s.hand.is_empty() && hand_value(&s.hand) < 21));
        match next {
            Some(seat) => self.turn = seat,
            None => self.end_player_turns(),
        }
    }

    /// The dealer only plays against a live hand, and a dealer already on 21 has nothing to decide.
    fn end_player_turns(&mut self) {
        if !self.players().any(|(_, p)| live(p)) || hand_value(&self.dealer().hand) == 21 {
            self.settle();
        } else {
            self.phase = Phase::DealerTurn;
            self.turn = DEALER;
        }
    }

    fn settle(&mut self) {
        let d = hand_value(&self.dealer().hand);
        let mut dealer_tally = self.dealer().tally;
        for s in self.seats[DEALER + 1..].iter_mut().flatten() {
            if s.hand.is_empty() {
                continue; // joined mid-round, was not dealt in
            }
            let p = hand_value(&s.hand);
            let r = if p > 21 {
                Outcome::Lose
            } else if d > 21 || p > d {
                Outcome::Win
            } else if p == d {
                Outcome::Push
            } else {
                Outcome::Lose
            };
            s.result = Some(r);
            s.tally.record(r);
            dealer_tally.record(r.opposite());
        }
        self.seat_mut(DEALER).tally = dealer_tally;
        self.phase = Phase::RoundOver;
        self.turn = DEALER;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P1: usize = 1;
    const P2: usize = 2;
    const P3: usize = 3;

    fn c(rank: u8) -> Card {
        Card { rank, suit: 0 }
    }

    fn hand(ranks: &[u8]) -> Vec<Card> {
        ranks.iter().copied().map(c).collect()
    }

    fn seat(ranks: &[u8]) -> Option<Seat> {
        Some(Seat { hand: hand(ranks), ..Default::default() })
    }

    /// A round already dealt with known cards: the dealer's, then players 1, 2, ... in order.
    /// It is player 1's turn. Tests draw from the end of their own shoe.
    fn dealt(dealer: &[u8], players: &[&[u8]]) -> GameState {
        let mut g = GameState::new();
        g.seats[DEALER] = seat(dealer);
        for (i, p) in players.iter().enumerate() {
            g.seats[i + 1] = seat(p);
        }
        g.phase = Phase::PlayerTurn;
        g.turn = P1;
        g.round = 1;
        g
    }

    /// A table between rounds with these players seated and nobody ready.
    fn table_of(players: &[usize]) -> GameState {
        let mut g = GameState::new();
        for &p in players {
            g.sit(p);
        }
        g
    }

    fn cards(g: &GameState, seat: usize) -> usize {
        g.seats[seat].as_ref().unwrap().hand.len()
    }

    fn value(g: &GameState, seat: usize) -> u8 {
        hand_value(&g.seats[seat].as_ref().unwrap().hand)
    }

    fn result(g: &GameState, seat: usize) -> Option<Outcome> {
        g.seats[seat].as_ref().unwrap().result
    }

    fn ready(g: &GameState, seat: usize) -> bool {
        g.seats[seat].as_ref().unwrap().ready
    }

    fn ready_seats(g: &GameState) -> Vec<usize> {
        g.seated().filter(|(_, s)| s.ready).map(|(i, _)| i).collect()
    }

    fn who_sits(g: &GameState) -> Vec<usize> {
        g.seated().map(|(i, _)| i).collect()
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
        let mut g = table_of(&[P1]);
        let mut shoe = vec![];
        g.apply(P1, Action::Hit, &mut shoe); // ignored, not started
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.apply(DEALER, Action::Ready, &mut shoe);
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!(g.round, 1);
        assert_eq!(shoe.len(), 52 - 4);
        assert_eq!((cards(&g, DEALER), cards(&g, P1)), (2, 2));
        assert_eq!(ready_seats(&g), [0usize; 0]);
        if value(&g, P1) != 21 {
            assert_eq!((g.phase, g.turn), (Phase::PlayerTurn, P1));
        }
    }

    #[test]
    fn dealer_plays_after_the_player_stands() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        let mut shoe = vec![c(5), c(2)]; // drawn from the end: 2, then 5
        g.apply(DEALER, Action::Hit, &mut shoe); // not the dealer's turn yet
        assert_eq!(cards(&g, DEALER), 2);
        g.apply(P1, Action::Stand, &mut shoe);
        assert_eq!(g.phase, Phase::DealerTurn);
        assert_eq!(g.to_act(), Some(DEALER));
        g.apply(P1, Action::Hit, &mut shoe); // player is done, ignored
        assert_eq!(cards(&g, P1), 2);
        g.apply(DEALER, Action::Hit, &mut shoe); // 14 + 2 = 16
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(DEALER, Action::Hit, &mut shoe); // 16 + 5 = 21, nothing left to decide
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Lose)); // 17 against 21
    }

    #[test]
    fn dealer_may_stand_short_and_lose() {
        let mut g = dealt(&[10, 7], &[&[10, 9]]);
        let mut shoe = vec![c(2)];
        g.apply(P1, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Stand, &mut shoe);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
        assert_eq!(shoe.len(), 1);
    }

    #[test]
    fn equal_totals_push() {
        let mut g = dealt(&[10, 8], &[&[10, 8]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        g.apply(DEALER, Action::Stand, &mut vec![]);
        assert_eq!(result(&g, P1), Some(Outcome::Push));
    }

    #[test]
    fn player_bust_ends_the_round_without_the_dealer_playing() {
        let mut g = dealt(&[2, 3], &[&[10, 6]]);
        let mut shoe = vec![c(10)];
        g.apply(P1, Action::Hit, &mut shoe); // 26
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Lose));
        assert_eq!(cards(&g, DEALER), 2);
    }

    #[test]
    fn dealer_bust_is_a_player_win() {
        let mut g = dealt(&[10, 6], &[&[10, 2]]);
        let mut shoe = vec![c(10)];
        g.apply(P1, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Hit, &mut shoe); // 26
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
    }

    #[test]
    fn dealer_natural_needs_no_decision() {
        let mut g = dealt(&[1, 13], &[&[10, 7]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Lose));
    }

    #[test]
    fn ready_is_ignored_mid_round() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        g.apply(DEALER, Action::Ready, &mut vec![]);
        assert_eq!(ready_seats(&g), [0usize; 0]);
        assert_eq!(g.phase, Phase::PlayerTurn);
    }

    #[test]
    fn card_labels_use_face_letters_and_suit_symbols() {
        let label = |rank, suit| Card { rank, suit }.label();
        assert_eq!(label(1, 0), "A♠");
        assert_eq!(label(2, 1), "2♥");
        assert_eq!(label(10, 2), "10♦");
        assert_eq!(label(11, 3), "J♣");
        assert_eq!(label(12, 0), "Q♠");
        assert_eq!(label(13, 1), "K♥");
        assert_eq!(label(7, 6), "7♦"); // an out-of-range suit wraps instead of panicking
    }

    #[test]
    fn an_empty_hand_is_worth_nothing() {
        assert_eq!(hand_value(&[]), 0);
    }

    #[test]
    fn aces_drop_to_one_only_as_far_as_needed() {
        assert_eq!(hand_value(&[c(1), c(5)]), 16); // soft 16
        assert_eq!(hand_value(&[c(1), c(5), c(10)]), 16); // the ace gives way
        assert_eq!(hand_value(&[c(1), c(1), c(9)]), 21); // one high, one low
        assert_eq!(hand_value(&[c(1), c(1), c(10), c(10)]), 22); // both low and still bust
    }

    #[test]
    fn a_new_game_is_the_dealer_alone_at_an_empty_table() {
        let g = GameState::new();
        assert_eq!(who_sits(&g), [DEALER]);
        assert_eq!(g.dealer(), &Seat::default());
        assert_eq!((g.phase, g.turn, g.round), (Phase::WaitingForReady, DEALER, 0));
        assert_eq!(g.to_act(), None);
        assert_eq!(g.seats.len(), 10, "a dealer and nine players");
    }

    #[test]
    fn a_fresh_shoe_is_one_full_deck() {
        let mut shoe = fresh_shoe();
        assert_eq!(shoe.len(), 52);
        assert!(shoe.iter().all(|c| (1..=13).contains(&c.rank) && c.suit < 4));
        shoe.sort_by_key(|c| (c.suit, c.rank));
        shoe.dedup();
        assert_eq!(shoe.len(), 52, "no duplicates");
    }

    #[test]
    fn the_shoe_is_shuffled() {
        // 52! orderings: two shoes in a row coming out identical means the shuffle is broken
        assert_ne!(fresh_shoe(), fresh_shoe());
    }

    #[test]
    fn an_empty_shoe_opens_a_fresh_deck_rather_than_running_out() {
        let mut shoe = vec![];
        let card = draw(&mut shoe);
        assert!((1..=13).contains(&card.rank) && card.suit < 4);
        assert_eq!(shoe.len(), 51);
    }

    #[test]
    fn one_seat_readying_twice_does_not_start_the_round() {
        let mut g = table_of(&[P1]);
        let mut shoe = vec![];
        g.apply(P1, Action::Ready, &mut shoe);
        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!(ready_seats(&g), [P1]);
        assert_eq!(g.phase, Phase::WaitingForReady);
        assert_eq!(g.round, 0);
        assert!(g.seated().all(|(_, s)| s.hand.is_empty()) && shoe.is_empty());
    }

    #[test]
    fn the_deal_comes_from_a_fresh_full_deck() {
        let mut g = table_of(&[P1]);
        let mut shoe = vec![c(1); 5]; // whatever was left from last round is thrown away
        g.apply(DEALER, Action::Ready, &mut shoe);
        g.apply(P1, Action::Ready, &mut shoe);
        let mut all: Vec<Card> = shoe.iter().copied().chain(g.seated().flat_map(|(_, s)| s.hand.iter().copied())).collect();
        assert_eq!(all.len(), 52);
        all.sort_by_key(|c| (c.suit, c.rank));
        all.dedup();
        assert_eq!(all.len(), 52, "dealt cards and the rest of the shoe make one deck");
    }

    #[test]
    fn a_dealt_natural_needs_no_player_decision() {
        let mut naturals = 0;
        for _ in 0..2000 {
            let mut g = table_of(&[P1]);
            let mut shoe = vec![];
            g.apply(DEALER, Action::Ready, &mut shoe);
            g.apply(P1, Action::Ready, &mut shoe);
            let (p, d) = (value(&g, P1), value(&g, DEALER));
            if p < 21 {
                assert_eq!((g.phase, g.turn), (Phase::PlayerTurn, P1), "{:?}", g.seats[P1]);
            } else {
                naturals += 1;
                if d == 21 {
                    assert_eq!((g.phase, result(&g, P1)), (Phase::RoundOver, Some(Outcome::Push)));
                } else {
                    assert_eq!((g.phase, result(&g, P1)), (Phase::DealerTurn, None));
                }
            }
        }
        assert!(naturals > 0, "about one deal in twenty is a natural");
    }

    #[test]
    fn hitting_to_exactly_21_ends_the_player_turn_without_a_bust() {
        let mut g = dealt(&[9, 5], &[&[10, 5]]);
        let mut shoe = vec![c(6)];
        g.apply(P1, Action::Hit, &mut shoe);
        assert_eq!(value(&g, P1), 21);
        assert_eq!((g.phase, result(&g, P1)), (Phase::DealerTurn, None));
    }

    #[test]
    fn a_three_card_21_pushes_against_a_dealer_natural() {
        let mut g = dealt(&[1, 13], &[&[10, 5]]);
        g.apply(P1, Action::Hit, &mut vec![c(6)]);
        assert_eq!((g.phase, result(&g, P1)), (Phase::RoundOver, Some(Outcome::Push)));
    }

    #[test]
    fn out_of_turn_actions_change_nothing() {
        let shoe = vec![c(2), c(3)];
        // player 1's turn: neither the dealer nor player 2 may hit or stand
        let mut g = dealt(&[9, 5], &[&[10, 7], &[8, 8]]);
        for seat in [DEALER, P2] {
            for a in [Action::Hit, Action::Stand] {
                let mut s = shoe.clone();
                g.apply(seat, a, &mut s);
                assert_eq!((g.phase, g.turn, cards(&g, seat), s.len()), (Phase::PlayerTurn, P1, 2, 2));
            }
        }
        // dealer's turn: the players are done
        g.apply(P1, Action::Stand, &mut shoe.clone());
        g.apply(P2, Action::Stand, &mut shoe.clone());
        assert_eq!(g.phase, Phase::DealerTurn);
        for seat in [P1, P2] {
            for a in [Action::Hit, Action::Stand] {
                let mut s = shoe.clone();
                g.apply(seat, a, &mut s);
                assert_eq!((g.phase, cards(&g, seat), s.len()), (Phase::DealerTurn, 2, 2));
            }
        }
        // round over: only Ready means anything
        g.apply(DEALER, Action::Stand, &mut shoe.clone());
        assert_eq!(g.phase, Phase::RoundOver);
        let before = g.clone();
        for seat in [DEALER, P1, P2] {
            for a in [Action::Hit, Action::Stand] {
                g.apply(seat, a, &mut shoe.clone());
            }
        }
        assert_eq!(g, before);
    }

    #[test]
    fn actions_from_an_empty_chair_change_nothing() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        let before = g.clone();
        for seat in [P2, MAX_PLAYERS, SEATS, 1000] {
            for a in [Action::Hit, Action::Stand, Action::Ready] {
                g.apply(seat, a, &mut vec![]);
            }
        }
        assert_eq!(g, before);
    }

    #[test]
    fn readying_after_a_round_keeps_the_result_up_until_everyone_agrees_then_deals_again() {
        let mut g = dealt(&[10, 7], &[&[10, 9]]);
        let mut shoe = vec![];
        g.apply(P1, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Stand, &mut shoe);
        assert_eq!((g.phase, result(&g, P1)), (Phase::RoundOver, Some(Outcome::Win)));

        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(ready_seats(&g), [P1]);
        assert_eq!(result(&g, P1), Some(Outcome::Win), "the result stays on the table");

        g.apply(DEALER, Action::Ready, &mut shoe);
        assert_eq!(g.round, 2);
        assert_eq!(ready_seats(&g), [0usize; 0]);
        assert_eq!(result(&g, P1), None);
        assert_eq!((cards(&g, DEALER), cards(&g, P1), shoe.len()), (2, 2, 48));
        assert_ne!(g.phase, Phase::WaitingForReady);
        assert_ne!(g.phase, Phase::RoundOver);
    }

    #[test]
    fn state_survives_a_json_round_trip() {
        let mut g = dealt(&[9, 5], &[&[1, 13], &[4, 4]]);
        g.seats[DEALER].as_mut().unwrap().hand.push(Card { rank: 12, suit: 3 });
        g.seats[P2].as_mut().unwrap().result = Some(Outcome::Win);
        g.seats[P2].as_mut().unwrap().ready = true;
        g.sit(7);
        g.phase = Phase::RoundOver;
        g.turn = 4;
        g.round = 42;
        let json = serde_json::to_string(&g).unwrap();
        assert_eq!(serde_json::from_str::<GameState>(&json).unwrap(), g);
    }

    // ---- more than one player ----------------------------------------------------------

    #[test]
    fn sitting_fills_a_chair_and_nothing_else() {
        let mut g = GameState::new();
        g.sit(P2);
        assert_eq!(who_sits(&g), [DEALER, P2]);
        assert_eq!(g.seats[P2], Some(Seat::default()));
        g.sit(MAX_PLAYERS);
        assert_eq!(who_sits(&g), [DEALER, P2, MAX_PLAYERS]);
        g.sit(SEATS); // off the end of the table
        g.sit(usize::MAX);
        assert_eq!(who_sits(&g), [DEALER, P2, MAX_PLAYERS]);
    }

    #[test]
    fn sitting_where_someone_already_sits_does_not_disturb_them() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        let before = g.clone();
        g.sit(P1);
        g.sit(DEALER);
        assert_eq!(g, before);
    }

    #[test]
    fn the_dealer_alone_cannot_start_a_round() {
        let mut g = GameState::new();
        let mut shoe = vec![];
        g.apply(DEALER, Action::Ready, &mut shoe);
        assert_eq!((g.phase, g.round, ready(&g, DEALER)), (Phase::WaitingForReady, 0, true));
        assert!(shoe.is_empty());
        // the dealer's ready stands: the first player to sit down and ready up starts it
        g.sit(P1);
        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!((g.phase, g.round), (Phase::PlayerTurn, 1));
    }

    #[test]
    fn the_round_waits_for_every_seat_to_be_ready() {
        let mut g = table_of(&[P1, P2, P3]);
        let mut shoe = vec![];
        for seat in [P3, DEALER, P1] {
            g.apply(seat, Action::Ready, &mut shoe);
            assert_eq!(g.phase, Phase::WaitingForReady, "still waiting on player 2");
        }
        assert_eq!(ready_seats(&g), [DEALER, P1, P3]);
        g.apply(P2, Action::Ready, &mut shoe);
        assert_eq!((g.phase, g.round), (Phase::PlayerTurn, 1));
        assert!(g.seated().all(|(_, s)| s.hand.len() == 2 && !s.ready && s.result.is_none()));
        assert_eq!(shoe.len(), 52 - 8);
    }

    #[test]
    fn players_act_in_seat_order_and_then_the_dealer() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6], &[5, 5]]);
        let mut shoe = vec![c(4), c(10)]; // drawn from the end: 10, then 4
        assert_eq!(g.to_act(), Some(P1));
        g.apply(P2, Action::Stand, &mut shoe); // not yet
        assert_eq!(g.to_act(), Some(P1));
        g.apply(P1, Action::Stand, &mut shoe);
        assert_eq!(g.to_act(), Some(P2));
        g.apply(P2, Action::Hit, &mut shoe); // 16 + 10 = bust, turn moves on
        assert_eq!((value(&g, P2), g.to_act()), (26, Some(P3)));
        g.apply(P3, Action::Hit, &mut shoe); // 10 + 4 = 14
        assert_eq!(g.to_act(), Some(P3));
        g.apply(P3, Action::Stand, &mut shoe);
        assert_eq!((g.phase, g.to_act()), (Phase::DealerTurn, Some(DEALER)));
        g.apply(DEALER, Action::Stand, &mut shoe); // 14 against 17, 26, 14
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
        assert_eq!(result(&g, P2), Some(Outcome::Lose));
        assert_eq!(result(&g, P3), Some(Outcome::Push));
        assert_eq!(g.dealer().result, None, "the dealer has no single result against a full table");
    }

    #[test]
    fn empty_chairs_between_players_are_skipped() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        g.seats[5] = seat(&[6, 6]);
        g.seats[9] = seat(&[7, 7]);
        g.apply(P1, Action::Stand, &mut vec![]);
        assert_eq!(g.to_act(), Some(5));
        g.apply(5, Action::Stand, &mut vec![]);
        assert_eq!(g.to_act(), Some(9));
        g.apply(9, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::DealerTurn);
    }

    #[test]
    fn a_player_dealt_a_natural_is_skipped() {
        let mut g = dealt(&[9, 5], &[&[1, 13], &[10, 6], &[1, 10]]);
        g.turn = DEALER;
        g.next_turn(); // what start_round does after dealing
        assert_eq!(g.to_act(), Some(P2), "players 1 and 3 have nothing to decide");
        g.apply(P2, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(DEALER, Action::Hit, &mut vec![c(3)]); // 17
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(DEALER, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
        assert_eq!(result(&g, P2), Some(Outcome::Lose));
        assert_eq!(result(&g, P3), Some(Outcome::Win));
    }

    #[test]
    fn everyone_busting_settles_without_the_dealer_playing() {
        let mut g = dealt(&[2, 3], &[&[10, 6], &[10, 7]]);
        g.apply(P1, Action::Hit, &mut vec![c(10)]); // 26
        assert_eq!(g.to_act(), Some(P2));
        g.apply(P2, Action::Hit, &mut vec![c(10)]); // 27
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(cards(&g, DEALER), 2);
        assert_eq!((result(&g, P1), result(&g, P2)), (Some(Outcome::Lose), Some(Outcome::Lose)));
    }

    #[test]
    fn one_live_hand_is_enough_to_make_the_dealer_play() {
        let mut g = dealt(&[2, 3], &[&[10, 6], &[10, 7]]);
        g.apply(P1, Action::Hit, &mut vec![c(10)]); // 26
        g.apply(P2, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::DealerTurn);
        g.apply(DEALER, Action::Hit, &mut vec![c(10)]); // 15
        g.apply(DEALER, Action::Hit, &mut vec![c(10)]); // 25
        assert_eq!((result(&g, P1), result(&g, P2)), (Some(Outcome::Lose), Some(Outcome::Win)));
    }

    #[test]
    fn a_dealer_bust_pays_every_live_hand_but_not_a_bust_one() {
        let mut g = dealt(&[10, 6], &[&[10, 2], &[10, 6], &[5, 4]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        g.apply(P2, Action::Hit, &mut vec![c(10)]); // 26
        g.apply(P3, Action::Stand, &mut vec![]);
        g.apply(DEALER, Action::Hit, &mut vec![c(10)]); // 26
        assert_eq!(result(&g, P1), Some(Outcome::Win));
        assert_eq!(result(&g, P2), Some(Outcome::Lose));
        assert_eq!(result(&g, P3), Some(Outcome::Win));
    }

    #[test]
    fn nine_players_fill_the_table_and_all_get_dealt() {
        let mut g = GameState::new();
        for seat in 1..=MAX_PLAYERS {
            g.sit(seat);
        }
        assert_eq!(who_sits(&g).len(), SEATS);
        let mut shoe = vec![];
        for seat in 0..SEATS {
            g.apply(seat, Action::Ready, &mut shoe);
        }
        assert_eq!(g.round, 1);
        assert!(g.seated().all(|(_, s)| s.hand.len() == 2));
        assert_eq!(shoe.len(), 52 - 20);
    }

    #[test]
    fn a_full_table_can_play_a_whole_round_hitting_freely() {
        // ten hands, everybody hits until they stand on 17 or bust: the deck runs out and a new one opens
        for _ in 0..200 {
            let mut g = GameState::new();
            for seat in 1..=MAX_PLAYERS {
                g.sit(seat);
            }
            let mut shoe = vec![];
            for seat in 0..SEATS {
                g.apply(seat, Action::Ready, &mut shoe);
            }
            while let Some(seat) = g.to_act() {
                let a = if value(&g, seat) < 17 { Action::Hit } else { Action::Stand };
                g.apply(seat, a, &mut shoe);
            }
            assert_eq!(g.phase, Phase::RoundOver);
            assert!(g.players().all(|(_, p)| p.result.is_some()));
        }
    }

    #[test]
    fn a_player_who_joins_mid_round_sits_out_until_the_next_deal() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        let mut shoe = vec![];
        g.sit(P2);
        assert_eq!(g.to_act(), Some(P1));
        g.apply(P2, Action::Hit, &mut shoe);
        g.apply(P2, Action::Ready, &mut shoe);
        assert_eq!(g.seats[P2], Some(Seat::default()), "no cards, not ready, nothing to hit with");
        g.apply(P1, Action::Stand, &mut shoe);
        assert_eq!((g.phase, g.to_act()), (Phase::DealerTurn, Some(DEALER)), "the newcomer is not asked to act");
        g.apply(DEALER, Action::Stand, &mut shoe);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
        assert_eq!(result(&g, P2), None, "no hand, no result");

        for seat in [DEALER, P1, P2] {
            g.apply(seat, Action::Ready, &mut shoe);
        }
        assert_eq!(g.round, 2);
        assert_eq!(cards(&g, P2), 2, "dealt in from the next round");
    }

    #[test]
    fn a_newcomer_busting_nobody_else_still_leaves_the_dealer_a_hand_to_beat() {
        // the dealer's turn only ends early when no dealt-in hand is live; an undealt seat is neither
        let mut g = dealt(&[9, 5], &[&[10, 6]]);
        g.sit(P2);
        g.apply(P1, Action::Hit, &mut vec![c(10)]); // 26, and the only dealt hand
        assert_eq!(g.phase, Phase::RoundOver, "nothing live for the dealer to play against");
    }

    #[test]
    fn leaving_frees_the_chair_and_the_dealer_cannot_leave() {
        let mut g = table_of(&[P1, P2]);
        let mut shoe = vec![];
        g.leave(P1, &mut shoe);
        assert_eq!(who_sits(&g), [DEALER, P2]);
        g.leave(P1, &mut shoe); // already gone
        g.leave(DEALER, &mut shoe);
        g.leave(SEATS, &mut shoe);
        assert_eq!(who_sits(&g), [DEALER, P2]);
        assert_eq!(g.phase, Phase::WaitingForReady);
    }

    #[test]
    fn a_player_leaving_on_their_turn_passes_it_on() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6]]);
        g.leave(P1, &mut vec![]);
        assert_eq!(who_sits(&g), [DEALER, P2]);
        assert_eq!(g.to_act(), Some(P2));
    }

    #[test]
    fn a_player_leaving_out_of_turn_changes_nothing_else() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6]]);
        g.leave(P2, &mut vec![]);
        assert_eq!((g.phase, g.to_act()), (Phase::PlayerTurn, Some(P1)));
        g.apply(P1, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::DealerTurn);
    }

    #[test]
    fn the_last_player_to_act_leaving_hands_the_round_to_the_dealer() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        g.leave(P2, &mut vec![]);
        assert_eq!((g.phase, g.to_act()), (Phase::DealerTurn, Some(DEALER)));
        g.apply(DEALER, Action::Stand, &mut vec![]);
        assert_eq!(result(&g, P1), Some(Outcome::Win));
    }

    #[test]
    fn everyone_leaving_mid_round_ends_it() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        g.leave(P1, &mut vec![]);
        assert_eq!((g.phase, who_sits(&g)), (Phase::RoundOver, vec![DEALER]));
    }

    #[test]
    fn the_last_live_hand_leaving_during_the_dealers_turn_settles_the_round() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        g.apply(P2, Action::Hit, &mut vec![c(10)]); // 26
        assert_eq!(g.phase, Phase::DealerTurn);
        g.leave(P1, &mut vec![]);
        assert_eq!(g.phase, Phase::RoundOver, "only a bust hand is left; nothing to play against");
        assert_eq!(result(&g, P2), Some(Outcome::Lose));
    }

    #[test]
    fn a_live_hand_leaving_during_the_dealers_turn_leaves_the_dealer_playing_the_rest() {
        let mut g = dealt(&[9, 5], &[&[10, 7], &[10, 6]]);
        g.apply(P1, Action::Stand, &mut vec![]);
        g.apply(P2, Action::Stand, &mut vec![]);
        g.leave(P1, &mut vec![]);
        assert_eq!((g.phase, g.to_act()), (Phase::DealerTurn, Some(DEALER)));
    }

    #[test]
    fn the_one_holdout_leaving_starts_the_round_the_others_were_ready_for() {
        let mut g = table_of(&[P1, P2]);
        let mut shoe = vec![];
        g.apply(DEALER, Action::Ready, &mut shoe);
        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!(g.phase, Phase::WaitingForReady);
        g.leave(P2, &mut shoe);
        assert_eq!((g.phase, g.round, who_sits(&g)), (Phase::PlayerTurn, 1, vec![DEALER, P1]));
    }

    #[test]
    fn the_last_player_leaving_does_not_start_a_round_for_the_dealer_alone() {
        let mut g = table_of(&[P1]);
        let mut shoe = vec![];
        g.apply(DEALER, Action::Ready, &mut shoe);
        g.leave(P1, &mut shoe);
        assert_eq!((g.phase, g.round, who_sits(&g)), (Phase::WaitingForReady, 0, vec![DEALER]));
    }

    #[test]
    fn to_act_names_the_seat_the_table_waits_on() {
        let mut g = dealt(&[9, 5], &[&[10, 7]]);
        assert_eq!(g.to_act(), Some(P1));
        g.turn = 7;
        assert_eq!(g.to_act(), Some(7));
        g.phase = Phase::DealerTurn;
        assert_eq!(g.to_act(), Some(DEALER));
        for phase in [Phase::WaitingForReady, Phase::RoundOver] {
            g.phase = phase;
            assert_eq!(g.to_act(), None);
        }
    }

    // ---- names and tallies ---------------------------------------------------------------

    fn tally(g: &GameState, seat: usize) -> (u32, u32, u32) {
        let t = g.seats[seat].as_ref().unwrap().tally;
        (t.wins, t.losses, t.pushes)
    }

    #[test]
    fn outcomes_flip_for_the_dealer() {
        assert_eq!(Outcome::Win.opposite(), Outcome::Lose);
        assert_eq!(Outcome::Lose.opposite(), Outcome::Win);
        assert_eq!(Outcome::Push.opposite(), Outcome::Push);
    }

    #[test]
    fn a_tally_counts_each_outcome_and_reads_as_w_l_p() {
        let mut t = Tally::default();
        assert_eq!(t.label(), "0W 0L 0P");
        for r in [Outcome::Win, Outcome::Win, Outcome::Lose, Outcome::Push] {
            t.record(r);
        }
        assert_eq!(t, Tally { wins: 2, losses: 1, pushes: 1 });
        assert_eq!(t.label(), "2W 1L 1P");
    }

    #[test]
    fn settling_credits_every_player_and_charges_the_dealer_the_opposite() {
        // dealer 17; P1 19 wins, P2 17 pushes, P3 busts, P4 joined mid-round and was not dealt in
        let mut g = dealt(&[10, 7], &[&[10, 9], &[10, 7], &[10, 9, 5]]);
        g.sit(4);
        for p in [P1, P2] {
            g.apply(p, Action::Stand, &mut vec![]);
        }
        assert_eq!(g.phase, Phase::DealerTurn, "{:?}", g.phase);
        g.apply(DEALER, Action::Stand, &mut vec![]);
        assert_eq!(g.phase, Phase::RoundOver);
        assert_eq!(tally(&g, P1), (1, 0, 0));
        assert_eq!(tally(&g, P2), (0, 0, 1));
        assert_eq!(tally(&g, P3), (0, 1, 0));
        assert_eq!(tally(&g, 4), (0, 0, 0), "not dealt in, nothing to count");
        assert_eq!(tally(&g, DEALER), (1, 1, 1), "one hand beaten, one lost, one pushed");
    }

    #[test]
    fn names_and_tallies_survive_the_next_deal() {
        let mut g = dealt(&[10, 7], &[&[10, 9]]);
        g.set_name(DEALER, "alice");
        g.set_name(P1, "bob");
        let mut shoe = vec![];
        g.apply(P1, Action::Stand, &mut shoe);
        g.apply(DEALER, Action::Stand, &mut shoe);
        assert_eq!(tally(&g, P1), (1, 0, 0));
        g.apply(DEALER, Action::Ready, &mut shoe);
        g.apply(P1, Action::Ready, &mut shoe);
        assert_eq!(g.round, 2);
        assert_eq!((g.dealer().name.as_str(), g.seats[P1].as_ref().unwrap().name.as_str()), ("alice", "bob"));
        assert_eq!(tally(&g, P1), (1, 0, 0), "the tally is not reset by the deal");
        assert_eq!(tally(&g, DEALER), (0, 1, 0));
        assert_eq!((cards(&g, P1), result(&g, P1), ready(&g, P1)), (2, None, false), "the hand itself is fresh");
    }

    #[test]
    fn a_name_needs_an_occupied_chair_and_a_freed_chair_forgets_it() {
        let mut g = table_of(&[P1]);
        g.set_name(P1, "bob");
        g.set_name(P2, "nobody"); // empty chair
        g.set_name(SEATS, "off the table");
        assert_eq!(g.seats[P1].as_ref().unwrap().name, "bob");
        assert_eq!(g.seats[P2], None);
        g.leave(P1, &mut vec![]);
        g.sit(P1);
        let fresh = g.seats[P1].as_ref().unwrap();
        assert_eq!((fresh.name.as_str(), fresh.tally), ("", Tally::default()), "the next person starts from nothing");
    }

    #[test]
    fn a_seat_without_a_name_or_tally_in_its_json_still_reads() {
        // the fields were added after the seat layout; a snapshot without them is a fresh seat
        let s: Seat = serde_json::from_str(r#"{"hand":[],"ready":true,"result":null}"#).unwrap();
        assert_eq!(s, Seat { ready: true, ..Default::default() });
    }
}
