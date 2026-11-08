//! A match (Bo1/Bo3 series) wrapping game states with rewind/forward history
//! and sideboarding. Used by the server for rooms and by the client for
//! offline solo/goldfish play, so both behave identically.

use crate::cards::CardDb;
use crate::decklist::{Deck, DeckRules};
use crate::game::{Action, GameOptions, GameState, Mode, NewSeat, Stage};
use crate::rng::Rng;
use serde::{Deserialize, Serialize};

pub const HISTORY_CAP: usize = 120;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Format {
    #[default]
    Bo1,
    Bo3,
}

impl Format {
    pub fn wins_needed(&self) -> u32 {
        match self {
            Format::Bo1 => 1,
            Format::Bo3 => 2,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Format::Bo1 => "Best of 1",
            Format::Bo3 => "Best of 3",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MatchConfig {
    pub format: Format,
    pub rules: DeckRules,
    pub solo: bool,
    pub auto_steps: bool,
    pub win_points: i32,
    #[serde(default)]
    pub mode: Mode,
}

impl Default for MatchConfig {
    fn default() -> Self {
        MatchConfig { format: Format::Bo1, rules: DeckRules::Standard, solo: false, auto_steps: true, win_points: crate::game::DEFAULT_WIN_POINTS, mode: Mode::Duel }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MatchStage {
    Playing,
    Sideboarding { locked: Vec<bool> },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MatchAction {
    Game(Action),
    Rewind,
    Forward,
    SideSwap { out_code: String, in_code: String },
    LockSideboard,
    NextGame,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MatchView {
    pub config: MatchConfig,
    pub names: Vec<String>,
    /// Series wins including the current game if it is over.
    pub wins: Vec<u32>,
    pub game_no: u32,
    pub stage: MatchStage,
    pub game: GameState,
    pub can_rewind: bool,
    pub can_forward: bool,
    pub next_ready: Vec<bool>,
    /// Set when the whole series is decided.
    pub match_winner: Option<u8>,
    /// Viewer's own current deck (for sideboarding).
    pub my_deck: Option<Deck>,
    pub viewer: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Match {
    pub config: MatchConfig,
    pub names: Vec<String>,
    pub decks: Vec<Deck>,
    pub wins: Vec<u32>,
    pub game_no: u32,
    pub used_bfs: Vec<Vec<String>>,
    pub stage: MatchStage,
    pub game: GameState,
    history: Vec<GameState>,
    future: Vec<GameState>,
    rng: Rng,
    next_ready: Vec<bool>,
}

fn options(c: &MatchConfig) -> GameOptions {
    GameOptions { auto_steps: c.auto_steps, solo: c.solo, win_points: c.win_points, mode: c.mode }
}

impl Match {
    pub fn new(db: &CardDb, mut config: MatchConfig, names: Vec<String>, decks: Vec<Deck>, seed: u64) -> Match {
        if config.mode != Mode::Duel {
            // Multiplayer games are single games.
            config.format = Format::Bo1;
        }
        let mut rng = Rng::new(seed);
        let n = names.len();
        let seats = names
            .iter()
            .zip(decks.iter())
            .map(|(nm, d)| NewSeat { name: nm.clone(), deck: d.clone(), bf_options: d.battlefields.clone() })
            .collect();
        let game = GameState::new(db, seats, options(&config), rng.next_u64(), None);
        Match { config, names, decks, wins: vec![0; n], game_no: 1, used_bfs: vec![vec![]; n], stage: MatchStage::Playing, game, history: vec![], future: vec![], rng, next_ready: vec![false; n] }
    }

    pub fn seats(&self) -> usize {
        self.names.len()
    }

    /// Series wins including the current (finished) game.
    pub fn series_wins(&self) -> Vec<u32> {
        let mut w = self.wins.clone();
        if let (Stage::Over, Some(x), MatchStage::Playing) = (&self.game.stage, self.game.winner, &self.stage) {
            w[x as usize] += 1;
        }
        w
    }

    pub fn match_winner(&self) -> Option<u8> {
        let need = self.config.format.wins_needed();
        self.series_wins().iter().position(|&w| w >= need).map(|i| i as u8)
    }

    pub fn apply(&mut self, db: &CardDb, actor: u8, action: MatchAction) -> Result<(), String> {
        if actor as usize >= self.seats() {
            return Err("Spectators cannot act in the match.".into());
        }
        match action {
            MatchAction::Game(a) => {
                if self.stage != MatchStage::Playing {
                    return Err("Finish sideboarding first.".into());
                }
                let snapshot = self.game.clone();
                self.game.apply(db, actor, a)?;
                self.history.push(snapshot);
                if self.history.len() > HISTORY_CAP {
                    self.history.remove(0);
                }
                self.future.clear();
                Ok(())
            }
            MatchAction::Rewind => {
                let prev = self.history.pop().ok_or("Nothing to rewind.")?;
                let cur = std::mem::replace(&mut self.game, prev);
                self.future.push(cur);
                let name = self.names[actor as usize].clone();
                self.game.log.push(crate::game::LogLine { seat: Some(actor), turn: self.game.turn, text: format!("{name} used Rewind to undo the last action.") });
                self.game.version = self.future.last().map(|f| f.version + 1).unwrap_or(self.game.version + 1);
                Ok(())
            }
            MatchAction::Forward => {
                let next = self.future.pop().ok_or("Nothing to restore.")?;
                let cur = std::mem::replace(&mut self.game, next);
                self.history.push(cur);
                self.game.log.push(crate::game::LogLine { seat: Some(actor), turn: self.game.turn, text: "Used Forward to restore the previous timeline.".into() });
                self.game.version += 1;
                Ok(())
            }
            MatchAction::NextGame => {
                if self.game.stage != Stage::Over || self.stage != MatchStage::Playing {
                    return Err("The current game is not over.".into());
                }
                if self.match_winner().is_some() {
                    return Err("The match is over.".into());
                }
                self.next_ready[actor as usize] = true;
                if self.config.solo {
                    self.next_ready.iter_mut().for_each(|r| *r = true);
                }
                if self.next_ready.iter().all(|&r| r) {
                    // Commit the result and move to sideboarding.
                    if let Some(w) = self.game.winner {
                        self.wins[w as usize] += 1;
                    }
                    for (i, s) in self.game.seats.iter().enumerate() {
                        if let Some(b) = &s.chosen_bf {
                            self.used_bfs[i].push(b.clone());
                        }
                    }
                    self.stage = MatchStage::Sideboarding { locked: vec![false; self.seats()] };
                }
                Ok(())
            }
            MatchAction::SideSwap { out_code, in_code } => {
                if !matches!(self.stage, MatchStage::Sideboarding { .. }) {
                    return Err("Sideboarding is only available between games.".into());
                }
                self.decks[actor as usize].swap(&out_code, &in_code)
            }
            MatchAction::LockSideboard => {
                let MatchStage::Sideboarding { locked } = &mut self.stage else { return Err("Sideboarding is not open.".into()) };
                locked[actor as usize] = true;
                if self.config.solo {
                    locked.iter_mut().for_each(|l| *l = true);
                }
                if locked.iter().all(|&l| l) {
                    self.start_next_game(db);
                }
                Ok(())
            }
        }
    }

    fn start_next_game(&mut self, db: &CardDb) {
        let loser = self.game.winner.map(|w| if self.seats() > 1 { (w + 1) % self.seats() as u8 } else { 0 });
        self.game_no += 1;
        let seats = (0..self.seats())
            .map(|i| {
                let d = &self.decks[i];
                let mut opts: Vec<String> = d.battlefields.iter().filter(|b| !self.used_bfs[i].contains(b)).cloned().collect();
                if opts.is_empty() {
                    opts = d.battlefields.clone();
                }
                NewSeat { name: self.names[i].clone(), deck: d.clone(), bf_options: opts }
            })
            .collect();
        self.game = GameState::new(db, seats, options(&self.config), self.rng.next_u64(), loser);
        self.history.clear();
        self.future.clear();
        self.next_ready = vec![false; self.seats()];
        self.stage = MatchStage::Playing;
    }

    pub fn view(&self, viewer: Option<u8>, spectators_see_hands: bool) -> MatchView {
        MatchView {
            config: self.config.clone(),
            names: self.names.clone(),
            wins: self.series_wins(),
            game_no: self.game_no,
            stage: self.stage.clone(),
            game: self.game.view_for(viewer, spectators_see_hands),
            can_rewind: !self.history.is_empty(),
            can_forward: !self.future.is_empty(),
            next_ready: self.next_ready.clone(),
            match_winner: self.match_winner(),
            my_deck: viewer.and_then(|v| self.decks.get(v as usize).cloned()),
            viewer,
        }
    }

    pub fn rules(&self) -> DeckRules {
        self.config.rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::tests::db;
    use crate::decklist::starter_deck;

    fn mk(format: Format) -> (CardDb, Match) {
        let db = db();
        let a = starter_deck(&db, "SFD-181").unwrap();
        let b = starter_deck(&db, "UNL-193").unwrap();
        let m = Match::new(&db, MatchConfig { format, ..Default::default() }, vec!["A".into(), "B".into()], vec![a, b], 9);
        (db, m)
    }

    fn play_setup(db: &CardDb, m: &mut Match) {
        for s in 0..2u8 {
            if m.game.seats[s as usize].chosen_bf.is_none() && m.game.stage == Stage::ChooseBattlefield {
                let b = m.game.seats[s as usize].bf_options[0].clone();
                m.apply(db, s, MatchAction::Game(Action::ChooseBattlefield { code: b })).unwrap();
            }
        }
        let Stage::ChooseOrder { chooser } = m.game.stage else { panic!("{:?}", m.game.stage) };
        m.apply(db, chooser, MatchAction::Game(Action::ChooseOrder { go_first: true })).unwrap();
        for s in 0..2 {
            m.apply(db, s, MatchAction::Game(Action::Mulligan { cards: vec![] })).unwrap();
        }
    }

    #[test]
    fn rewind_and_forward() {
        let (db, mut m) = mk(Format::Bo1);
        play_setup(&db, &mut m);
        let a = m.game.active;
        let hand = m.game.hand(a).len();
        m.apply(&db, a, MatchAction::Game(Action::Draw { n: 1 })).unwrap();
        assert_eq!(m.game.hand(a).len(), hand + 1);
        m.apply(&db, a, MatchAction::Rewind).unwrap();
        assert_eq!(m.game.hand(a).len(), hand);
        m.apply(&db, a, MatchAction::Forward).unwrap();
        assert_eq!(m.game.hand(a).len(), hand + 1);
        assert!(m.apply(&db, a, MatchAction::Forward).is_err());
        assert!(m.apply(&db, 5, MatchAction::Rewind).is_err());
    }

    #[test]
    fn bo3_series() {
        let (db, mut m) = mk(Format::Bo3);
        play_setup(&db, &mut m);
        let first_bf = m.game.seats[0].chosen_bf.clone().unwrap();
        m.apply(&db, 1, MatchAction::Game(Action::Concede)).unwrap();
        assert_eq!(m.series_wins(), vec![1, 0]);
        assert_eq!(m.match_winner(), None);
        // concede misclick can be rewound
        m.apply(&db, 1, MatchAction::Rewind).unwrap();
        assert_eq!(m.series_wins(), vec![0, 0]);
        m.apply(&db, 1, MatchAction::Forward).unwrap();
        m.apply(&db, 0, MatchAction::NextGame).unwrap();
        assert_eq!(m.stage, MatchStage::Playing);
        m.apply(&db, 1, MatchAction::NextGame).unwrap();
        assert!(matches!(m.stage, MatchStage::Sideboarding { .. }));
        assert!(m.apply(&db, 0, MatchAction::Game(Action::Draw { n: 1 })).is_err());
        m.apply(&db, 0, MatchAction::LockSideboard).unwrap();
        m.apply(&db, 1, MatchAction::LockSideboard).unwrap();
        assert_eq!(m.game_no, 2);
        assert!(!m.game.seats[0].bf_options.contains(&first_bf));
        // loser (seat 1) chooses order
        for s in 0..2u8 {
            let b = m.game.seats[s as usize].bf_options[0].clone();
            m.apply(&db, s, MatchAction::Game(Action::ChooseBattlefield { code: b })).unwrap();
        }
        assert_eq!(m.game.stage, Stage::ChooseOrder { chooser: 1 });
        m.apply(&db, 1, MatchAction::Game(Action::ChooseOrder { go_first: true })).unwrap();
        for s in 0..2 {
            m.apply(&db, s, MatchAction::Game(Action::Mulligan { cards: vec![] })).unwrap();
        }
        m.apply(&db, 1, MatchAction::Game(Action::Concede)).unwrap();
        assert_eq!(m.match_winner(), Some(0));
        assert!(m.apply(&db, 0, MatchAction::NextGame).is_err());
        let v = m.view(Some(0), false);
        assert_eq!(v.wins, vec![2, 0]);
        assert!(v.my_deck.is_some());
        assert!(m.view(None, false).my_deck.is_none());
    }
}
