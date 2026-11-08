//! Tabletop game engine for Riftbound.
//!
//! The engine models zones, card state and turn structure and provides
//! automation helpers (turn steps, scoring, auto-pay, combat cleanup), while
//! still allowing free manual board control like a physical table.
//! All randomness comes from the state's own PRNG so snapshots can be rewound.

use crate::cards::CardDb;
use crate::decklist::Deck;
use crate::rng::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type CardId = u32;
pub const DEFAULT_WIN_POINTS: i32 = 8;
pub const TEAM_WIN_POINTS: i32 = 11;

/// How many players sit at the table and who is on whose side.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Mode {
    /// 1v1.
    #[default]
    Duel,
    /// 2v2 ("Magma Chamber"): seats 0 and 2 are Team A, seats 1 and 3 are Team B,
    /// so turns alternate between the teams. Teammates share one score.
    Teams,
    /// Four-player free-for-all ("War").
    FreeForAll,
    /// Three-player free-for-all ("Skirmish").
    Skirmish,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Duel, Mode::Teams, Mode::Skirmish, Mode::FreeForAll];
    pub fn seats(&self) -> usize {
        match self {
            Mode::Duel => 2,
            Mode::Skirmish => 3,
            Mode::Teams | Mode::FreeForAll => 4,
        }
    }
    /// Every player for themselves with more than two seats.
    pub fn is_ffa(&self) -> bool {
        matches!(self, Mode::FreeForAll | Mode::Skirmish)
    }
    pub fn default_points(&self) -> i32 {
        match self {
            Mode::Teams => TEAM_WIN_POINTS,
            _ => DEFAULT_WIN_POINTS,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Duel => "1v1",
            Mode::Teams => "2v2",
            Mode::FreeForAll => "Free-for-all (4)",
            Mode::Skirmish => "Free-for-all (3)",
        }
    }
    pub fn key(&self) -> &'static str {
        match self {
            Mode::Duel => "duel",
            Mode::Teams => "teams",
            Mode::FreeForAll => "ffa",
            Mode::Skirmish => "ffa3",
        }
    }
    pub fn from_key(k: &str) -> Mode {
        match k {
            "teams" => Mode::Teams,
            "ffa" => Mode::FreeForAll,
            "ffa3" => Mode::Skirmish,
            _ => Mode::Duel,
        }
    }
}

/// Team of a seat: 2v2 teams are seat parity, otherwise every seat is its own team.
pub fn team_of(mode: Mode, seat: u8) -> u8 {
    if mode == Mode::Teams {
        seat % 2
    } else {
        seat
    }
}

pub fn team_name(team: u8) -> &'static str {
    if team == 0 {
        "Team A"
    } else {
        "Team B"
    }
}
pub const OPENING_HAND: usize = 4;
pub const MULLIGAN_MAX: usize = 2;
pub const LOG_CAP: usize = 400;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Zone {
    Hand(u8),
    Deck(u8),
    RuneDeck(u8),
    Runes(u8),
    Base(u8),
    Trash(u8),
    Banish(u8),
    Legend(u8),
    Champion(u8),
    Side(u8),
    Reveal(u8),
    /// Unit lane of `seat` at battlefield `bf`.
    Bf(u8, u8),
    /// The battlefield card itself.
    BfCard(u8),
    Chain,
    /// Removed from the game (tokens that left the board).
    Void,
}

impl Zone {
    pub fn key(&self) -> String {
        match self {
            Zone::Hand(s) => format!("hand{s}"),
            Zone::Deck(s) => format!("deck{s}"),
            Zone::RuneDeck(s) => format!("runedeck{s}"),
            Zone::Runes(s) => format!("runes{s}"),
            Zone::Base(s) => format!("base{s}"),
            Zone::Trash(s) => format!("trash{s}"),
            Zone::Banish(s) => format!("banish{s}"),
            Zone::Legend(s) => format!("legend{s}"),
            Zone::Champion(s) => format!("champion{s}"),
            Zone::Side(s) => format!("side{s}"),
            Zone::Reveal(s) => format!("reveal{s}"),
            Zone::Bf(b, s) => format!("bf{b}_{s}"),
            Zone::BfCard(b) => format!("bfcard{b}"),
            Zone::Chain => "chain".into(),
            Zone::Void => "void".into(),
        }
    }
    /// The seat that owns this zone, if any.
    pub fn seat(&self) -> Option<u8> {
        match *self {
            Zone::Hand(s) | Zone::Deck(s) | Zone::RuneDeck(s) | Zone::Runes(s) | Zone::Base(s) | Zone::Trash(s) | Zone::Banish(s) | Zone::Legend(s) | Zone::Champion(s) | Zone::Side(s) | Zone::Reveal(s) | Zone::Bf(_, s) => Some(s),
            _ => None,
        }
    }
    /// Hidden zones: contents are private to the owner (or to nobody).
    pub fn is_hidden(&self) -> bool {
        matches!(self, Zone::Hand(_) | Zone::Deck(_) | Zone::RuneDeck(_) | Zone::Side(_))
    }
    /// Cards on the board (where exhausted/damage etc. matter).
    pub fn is_board(&self) -> bool {
        matches!(self, Zone::Base(_) | Zone::Bf(_, _) | Zone::Runes(_) | Zone::Legend(_) | Zone::Champion(_) | Zone::BfCard(_))
    }
    pub fn is_unit_area(&self) -> bool {
        matches!(self, Zone::Base(_) | Zone::Bf(_, _))
    }
    pub fn label(&self) -> &'static str {
        match self {
            Zone::Hand(_) => "hand",
            Zone::Deck(_) => "main deck",
            Zone::RuneDeck(_) => "rune deck",
            Zone::Runes(_) => "rune pool",
            Zone::Base(_) => "base",
            Zone::Trash(_) => "trash",
            Zone::Banish(_) => "banishment",
            Zone::Legend(_) => "legend zone",
            Zone::Champion(_) => "champion zone",
            Zone::Side(_) => "sideboard",
            Zone::Reveal(_) => "revealed area",
            Zone::Bf(_, _) => "battlefield",
            Zone::BfCard(_) => "battlefield",
            Zone::Chain => "chain",
            Zone::Void => "void",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CardInst {
    pub id: CardId,
    /// Empty string when hidden from the viewer.
    pub code: String,
    pub owner: u8,
    pub zone: Zone,
    pub exhausted: bool,
    pub face_down: bool,
    pub damage: i32,
    pub buffs: i32,
    pub counters: i32,
    pub stunned: bool,
    pub token: bool,
    /// Revealed to everyone while in a hidden zone (e.g. revealed hand card).
    pub shown: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerState {
    pub name: String,
    pub score: i32,
    pub bf_options: Vec<String>,
    pub chosen_bf: Option<String>,
    pub mulligan_done: bool,
    pub hand_shown: bool,
    pub roll: Option<u32>,
    pub had_turn: bool,
    pub conceded: bool,
    pub extra_turns: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Battlefield {
    pub card: CardId,
    pub controller: Option<u8>,
    /// Turn number each seat last scored here (conquer or hold).
    pub scored: Vec<u32>,
    pub contested_by: Option<u8>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Awaken,
    Beginning,
    Channel,
    Draw,
    Action,
    End,
}

impl Phase {
    pub const ALL: [Phase; 6] = [Phase::Awaken, Phase::Beginning, Phase::Channel, Phase::Draw, Phase::Action, Phase::End];
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Awaken => "Awaken",
            Phase::Beginning => "Beginning",
            Phase::Channel => "Channel",
            Phase::Draw => "Draw",
            Phase::Action => "Action",
            Phase::End => "End",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Stage {
    ChooseBattlefield,
    ChooseOrder { chooser: u8 },
    Mulligan,
    Play,
    Over,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Showdown {
    pub bf: u8,
    pub focus: u8,
    pub combat: bool,
    pub passes: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Peek {
    pub seat: u8,
    pub cards: Vec<CardId>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LogLine {
    pub seat: Option<u8>,
    pub turn: u32,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ping {
    pub card: CardId,
    pub seat: u8,
    pub nonce: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GameOptions {
    /// Automatically run Awaken/Beginning/Channel/Draw at turn start.
    pub auto_steps: bool,
    /// One client controls every seat (solo play / goldfish).
    pub solo: bool,
    pub win_points: i32,
    #[serde(default)]
    pub mode: Mode,
}

impl Default for GameOptions {
    fn default() -> Self {
        GameOptions { auto_steps: true, solo: false, win_points: DEFAULT_WIN_POINTS, mode: Mode::Duel }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GameState {
    pub seats: Vec<PlayerState>,
    pub cards: Vec<CardInst>,
    pub piles: BTreeMap<String, Vec<CardId>>,
    pub battlefields: Vec<Battlefield>,
    pub stage: Stage,
    pub turn: u32,
    pub active: u8,
    pub first: u8,
    pub phase: Phase,
    pub showdown: Option<Showdown>,
    pub winner: Option<u8>,
    pub log: Vec<LogLine>,
    pub peeks: Vec<Peek>,
    pub ping: Option<Ping>,
    pub options: GameOptions,
    pub rng: u64,
    pub version: u64,
    pub last_roll: Option<(u8, u32)>,
    /// Bo3: the previous game's loser picks turn order instead of rolling.
    pub order_chooser: Option<u8>,
}

/// Where a moved card lands within its destination pile.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pos {
    /// End of the list (top of a deck, right of a row).
    #[default]
    Top,
    /// Start of the list (bottom of a deck).
    Bottom,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Action {
    // ---- setup ----
    ChooseBattlefield { code: String },
    ChooseOrder { go_first: bool },
    Mulligan { cards: Vec<CardId> },
    // ---- turn structure ----
    StartTurn,
    AdvancePhase,
    Awaken,
    ScoreHolds,
    Channel { n: u32 },
    Draw { n: u32 },
    EndTurn,
    TimeWarp,
    // ---- card manipulation ----
    Move { card: CardId, to: Zone, #[serde(default)] pos: Pos },
    PlayCard { card: CardId, to: Option<Zone>, autopay: bool },
    AutoPay { card: CardId },
    Hide { card: CardId, bf: u8 },
    SetExhausted { card: CardId, exhausted: bool },
    ToggleExhausted { card: CardId },
    Flip { card: CardId },
    Damage { card: CardId, delta: i32 },
    Buff { card: CardId, delta: i32 },
    Counter { card: CardId, delta: i32 },
    Stun { card: CardId },
    Kill { card: CardId },
    Recall { card: CardId },
    Banish { card: CardId },
    Reveal { card: CardId },
    RevealHand { shown: bool },
    CreateToken { code: String, to: Zone, count: u32 },
    Ping { card: CardId },
    // ---- decks ----
    Shuffle { zone: Zone },
    Peek { zone: Zone, n: u32, #[serde(default)] bottom: bool },
    Search { zone: Zone },
    ClosePeek,
    Burn { n: u32 },
    RevealTop { zone: Zone },
    RecycleRune { card: CardId },
    ReadyRunes,
    // ---- scoring / battlefields ----
    Score { seat: u8, delta: i32 },
    Conquer { bf: u8 },
    Hold { bf: u8 },
    SetController { bf: u8, seat: Option<u8> },
    // ---- showdowns / combat / chain ----
    StartShowdown { bf: u8 },
    PassFocus,
    EndShowdown,
    ResolveCombat { bf: u8 },
    CleanupLethal,
    ApproveChain,
    // ---- misc ----
    RollD20,
    Concede,
}

impl Action {
    /// Actions that only the acting seat should be able to see the result of are
    /// still logged; this flags actions that never change shared state (none today).
    pub fn is_setup(&self) -> bool {
        matches!(self, Action::ChooseBattlefield { .. } | Action::ChooseOrder { .. } | Action::Mulligan { .. })
    }
}

pub struct NewSeat {
    pub name: String,
    pub deck: Deck,
    /// Battlefields this seat may still choose from (Bo3 excludes used ones).
    pub bf_options: Vec<String>,
}

type R = Result<(), String>;

impl GameState {
    /// Build a fresh game. `chooser` (Bo3 games 2-3: previous loser) skips the d20 roll.
    pub fn new(db: &CardDb, seats: Vec<NewSeat>, options: GameOptions, seed: u64, chooser: Option<u8>) -> GameState {
        let mut g = GameState {
            seats: vec![],
            cards: vec![],
            piles: BTreeMap::new(),
            battlefields: vec![],
            stage: Stage::ChooseBattlefield,
            turn: 0,
            active: 0,
            first: 0,
            phase: Phase::Awaken,
            showdown: None,
            winner: None,
            log: vec![],
            peeks: vec![],
            ping: None,
            options,
            rng: Rng::new(seed).next_u64(),
            version: 0,
            last_roll: None,
            order_chooser: chooser,
        };
        for (i, s) in seats.into_iter().enumerate() {
            let seat = i as u8;
            g.seats.push(PlayerState { name: s.name.clone(), bf_options: s.bf_options.clone(), ..Default::default() });
            let d = &s.deck;
            if let Some(l) = &d.legend {
                g.spawn(l, seat, Zone::Legend(seat), false);
            }
            if let Some(c) = &d.champion {
                g.spawn(c, seat, Zone::Champion(seat), false);
            }
            for (c, n) in &d.main {
                for _ in 0..*n {
                    g.spawn(c, seat, Zone::Deck(seat), false);
                }
            }
            for (c, n) in &d.runes {
                for _ in 0..*n {
                    g.spawn(c, seat, Zone::RuneDeck(seat), false);
                }
            }
            for (c, n) in &d.side {
                for _ in 0..*n {
                    g.spawn(c, seat, Zone::Side(seat), false);
                }
            }
            g.shuffle_zone(Zone::Deck(seat));
            g.shuffle_zone(Zone::RuneDeck(seat));
        }
        if g.multi() {
            // 2v2 and free-for-all: turn order comes first, because the first
            // player does not bring a battlefield.
            g.sys_log("The game is starting. Rolling for turn order.");
            g.roll_for_order();
            return g;
        }
        g.sys_log("The game is starting. Choose battlefields.");
        // Auto-choose when a seat has 0 or 1 options.
        for s in 0..g.seats.len() {
            let opts = g.seats[s].bf_options.clone();
            if opts.len() == 1 {
                g.seats[s].chosen_bf = Some(opts[0].clone());
                g.log_seat(s as u8, "Auto-selected the last remaining battlefield.");
            }
        }
        g.check_battlefields_done(db);
        g
    }

    // ------------------------------------------------------------------ helpers

    fn spawn(&mut self, code: &str, owner: u8, zone: Zone, token: bool) -> CardId {
        let id = self.cards.len() as CardId;
        self.cards.push(CardInst { id, code: code.to_string(), owner, zone, exhausted: false, face_down: false, damage: 0, buffs: 0, counters: 0, stunned: false, token, shown: false });
        self.piles.entry(zone.key()).or_default().push(id);
        id
    }

    pub fn pile(&self, z: Zone) -> &[CardId] {
        self.piles.get(&z.key()).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn pile_mut(&mut self, z: Zone) -> &mut Vec<CardId> {
        self.piles.entry(z.key()).or_default()
    }

    pub fn card(&self, id: CardId) -> Option<&CardInst> {
        self.cards.get(id as usize)
    }

    fn card_mut(&mut self, id: CardId) -> Result<&mut CardInst, String> {
        self.cards.get_mut(id as usize).ok_or_else(|| "That card does not exist.".to_string())
    }

    fn rng(&mut self) -> Rng {
        Rng(self.rng)
    }

    fn sys_log(&mut self, text: &str) {
        self.push_log(None, text.to_string());
    }
    fn log_seat(&mut self, seat: u8, text: &str) {
        self.push_log(Some(seat), text.to_string());
    }
    fn push_log(&mut self, seat: Option<u8>, text: String) {
        // Merge runs like "Channeled 1 rune." x3 into "Channeled 3 runes."
        if let Some(last) = self.log.last_mut() {
            if last.seat == seat && last.turn == self.turn {
                for (verb, noun) in [("Channeled", "rune"), ("Drew", "card"), ("Burned", "card")] {
                    if let (Some(a), Some(b)) = (count_of(&last.text, verb, noun), count_of(&text, verb, noun)) {
                        let n = a + b;
                        last.text = format!("{verb} {n} {noun}s.");
                        return;
                    }
                }
            }
        }
        self.log.push(LogLine { seat, turn: self.turn, text });
        if self.log.len() > LOG_CAP {
            let drop = self.log.len() - LOG_CAP;
            self.log.drain(0..drop);
        }
    }

    fn name_of(&self, db: &CardDb, id: CardId) -> String {
        self.card(id).and_then(|c| db.get(&c.code)).map(|d| d.name.clone()).unwrap_or_else(|| "a card".into())
    }

    pub fn num_seats(&self) -> u8 {
        self.seats.len() as u8
    }

    /// 2v2 or free-for-all with more than two seats.
    pub fn multi(&self) -> bool {
        self.options.mode != Mode::Duel && self.seats.len() > 2
    }

    pub fn team(&self, seat: u8) -> u8 {
        team_of(self.options.mode, seat)
    }

    /// Same seat or teammate.
    pub fn allied(&self, a: u8, b: u8) -> bool {
        self.team(a) == self.team(b)
    }

    /// Did `seat` (or its team) win?
    pub fn won(&self, seat: u8) -> bool {
        self.winner.map(|w| self.allied(w, seat)).unwrap_or(false)
    }

    /// Seats that take their turn last in the rotation.
    pub fn last_seat(&self) -> u8 {
        let n = self.num_seats().max(1);
        (self.first + n - 1) % n
    }

    /// Display name for a seat's side: "Ann" or "Ann & Cy" for a 2v2 team.
    pub fn side_name(&self, seat: u8) -> String {
        if self.options.mode == Mode::Teams {
            let t = self.team(seat);
            (0..self.num_seats()).filter(|&s| self.team(s) == t).map(|s| self.seats[s as usize].name.clone()).collect::<Vec<_>>().join(" & ")
        } else {
            self.seats[seat as usize].name.clone()
        }
    }

    /// One seat per opposing side that is still in the game.
    fn opposing_reps(&self, seat: u8) -> Vec<u8> {
        let mut teams = vec![];
        let mut out = vec![];
        for o in 0..self.num_seats() {
            let t = self.team(o);
            if self.allied(o, seat) || self.seats[o as usize].conceded || teams.contains(&t) {
                continue;
            }
            teams.push(t);
            out.push(o);
        }
        out
    }

    /// Next seat after `a` that has not conceded.
    fn next_seat(&self, a: u8) -> u8 {
        let n = self.num_seats();
        let mut s = (a + 1) % n;
        while s != a && self.seats[s as usize].conceded {
            s = (s + 1) % n;
        }
        s
    }

    pub fn might(&self, db: &CardDb, id: CardId) -> i32 {
        let c = &self.cards[id as usize];
        db.get(&c.code).and_then(|d| d.might).unwrap_or(0) + c.buffs
    }

    /// Detach a card from its pile and put it into `to`.
    fn place(&mut self, id: CardId, to: Zone, pos: Pos) {
        let from = self.cards[id as usize].zone;
        if let Some(p) = self.piles.get_mut(&from.key()) {
            p.retain(|&x| x != id);
        }
        for pk in &mut self.peeks {
            pk.cards.retain(|&x| x != id);
        }
        let token = self.cards[id as usize].token;
        let to = if token && !to.is_board() && to != Zone::Chain { Zone::Void } else { to };
        {
            let c = &mut self.cards[id as usize];
            c.zone = to;
            if !to.is_board() {
                c.exhausted = false;
                c.damage = 0;
                c.buffs = 0;
                c.counters = 0;
                c.stunned = false;
                c.face_down = false;
            }
            if !to.is_hidden() || matches!(to, Zone::Deck(_) | Zone::RuneDeck(_)) {
                c.shown = false;
            }
        }
        if to == Zone::Void {
            return;
        }
        let p = self.pile_mut(to);
        match pos {
            Pos::Top => p.push(id),
            Pos::Bottom => p.insert(0, id),
        }
    }

    /// Shuffle by permuting card *contents* among the pile's ids so that
    /// opponents cannot track a known card id through a shuffle.
    fn shuffle_zone(&mut self, z: Zone) {
        let ids: Vec<CardId> = self.pile(z).to_vec();
        let mut payload: Vec<CardInst> = ids.iter().map(|&i| self.cards[i as usize].clone()).collect();
        let mut r = self.rng();
        r.shuffle(&mut payload);
        self.rng = r.0;
        for (slot, mut c) in ids.iter().zip(payload) {
            c.id = *slot;
            c.shown = false;
            self.cards[*slot as usize] = c;
        }
        self.peeks.iter_mut().for_each(|p| p.cards.retain(|id| !ids.contains(id)));
    }

    fn may_touch(&self, actor: u8, id: CardId) -> R {
        if self.options.solo {
            return Ok(());
        }
        let c = self.card(id).ok_or("That card does not exist.")?;
        let private = c.zone.is_hidden() || c.face_down || c.zone == Zone::Reveal(c.owner) && false;
        if private && c.owner != actor && c.zone.seat() != Some(actor) {
            return Err("You cannot touch another player's hidden cards.".into());
        }
        Ok(())
    }

    fn own_zone(&self, actor: u8, z: Zone) -> R {
        if self.options.solo {
            return Ok(());
        }
        if z.is_hidden() && z.seat() != Some(actor) {
            return Err("That zone belongs to another player.".into());
        }
        Ok(())
    }

    fn check_seat(&self, s: u8) -> R {
        if (s as usize) < self.seats.len() {
            Ok(())
        } else {
            Err("Unknown seat.".into())
        }
    }

    fn draw_one(&mut self, seat: u8) {
        if self.pile(Zone::Deck(seat)).is_empty() {
            // Burn Out: shuffle trash into the deck and each opponent scores 1.
            let trash: Vec<CardId> = self.pile(Zone::Trash(seat)).to_vec();
            for id in trash {
                self.place(id, Zone::Deck(seat), Pos::Top);
            }
            self.shuffle_zone(Zone::Deck(seat));
            self.log_seat(seat, "Burned out: shuffled their trash into their deck.");
            for o in self.opposing_reps(seat) {
                self.add_score(o, 1, "Scored 1 from an opponent's burn out");
            }
            if self.pile(Zone::Deck(seat)).is_empty() {
                return;
            }
        }
        if let Some(&top) = self.pile(Zone::Deck(seat)).last() {
            self.place(top, Zone::Hand(seat), Pos::Top);
        }
    }

    fn channel(&mut self, seat: u8, n: u32) -> u32 {
        let mut done = 0;
        for _ in 0..n {
            if let Some(&top) = self.pile(Zone::RuneDeck(seat)).last() {
                self.place(top, Zone::Runes(seat), Pos::Top);
                done += 1;
            }
        }
        done
    }

    fn add_score(&mut self, seat: u8, delta: i32, msg: &str) {
        let score = (self.seats[seat as usize].score + delta).max(0);
        // Teammates share one score, stored on every member.
        for o in 0..self.num_seats() {
            if self.allied(o, seat) {
                self.seats[o as usize].score = score;
            }
        }
        self.log_seat(seat, &format!("{msg} ({score} point{}).", if score == 1 { "" } else { "s" }));
        self.check_winner();
    }

    fn check_winner(&mut self) {
        if self.winner.is_some() {
            return;
        }
        // win points are evaluated against the stored option (db-free); battlefields
        // that raise the target adjust options.win_points when they enter play.
        let target = self.options.win_points;
        if let Some((i, _)) = self.seats.iter().enumerate().find(|(_, s)| s.score >= target) {
            self.finish(i as u8, "reached the victory score");
        }
    }

    fn finish(&mut self, winner: u8, why: &str) {
        self.winner = Some(winner);
        self.stage = Stage::Over;
        self.showdown = None;
        let name = self.side_name(winner);
        let verb = if self.options.mode == Mode::Teams { "win" } else { "wins" };
        self.sys_log(&format!("{name} {verb} the game ({why})."));
    }

    fn check_battlefields_done(&mut self, db: &CardDb) {
        let chooser = self.order_chooser;
        if self.stage != Stage::ChooseBattlefield {
            return;
        }
        let multi = self.multi();
        let first = self.first;
        if !self.seats.iter().enumerate().all(|(i, s)| (multi && i as u8 == first) || s.chosen_bf.is_some() || s.bf_options.is_empty()) {
            return;
        }
        let n = self.num_seats();
        let order: Vec<u8> = if multi { (1..n).map(|k| (first + k) % n).collect() } else { (0..n).collect() };
        let mut chosen: Vec<(u8, String)> = order.iter().filter_map(|&i| self.seats[i as usize].chosen_bf.clone().map(|c| (i, c))).collect();
        // Goldfish: a lone player still plays with two battlefields.
        if self.seats.len() == 1 {
            let extra = self.seats[0].bf_options.iter().find(|b| Some(*b) != self.seats[0].chosen_bf.as_ref()).cloned();
            if let Some(e) = extra {
                chosen.push((0, e));
            }
        }
        for (i, (owner, code)) in chosen.into_iter().enumerate() {
            let id = self.spawn(&code, owner, Zone::BfCard(i as u8), false);
            self.battlefields.push(Battlefield { card: id, controller: None, scored: vec![0; self.seats.len()], contested_by: None });
            if db.get(&code).map(|d| d.text.contains("Increase the points needed to win")).unwrap_or(false) {
                self.options.win_points += 1;
            }
        }
        self.sys_log("All battlefields are locked.");
        if multi {
            self.begin_mulligan();
            return;
        }
        match chooser {
            Some(c) => {
                self.stage = Stage::ChooseOrder { chooser: c };
                let n = self.seats[c as usize].name.clone();
                self.sys_log(&format!("{n} lost the previous game and chooses to play first or second."));
            }
            None => self.roll_for_order(),
        }
    }

    fn roll_for_order(&mut self) {
        let n = self.seats.len();
        if n == 1 {
            self.stage = Stage::ChooseOrder { chooser: 0 };
            return;
        }
        let mut r = self.rng();
        loop {
            let rolls: Vec<u32> = (0..n).map(|_| r.below(20) as u32 + 1).collect();
            let max = *rolls.iter().max().unwrap();
            for (i, v) in rolls.iter().enumerate() {
                self.seats[i].roll = Some(*v);
            }
            let line = rolls.iter().enumerate().map(|(i, v)| format!("{} rolled {}", self.seats[i].name, v)).collect::<Vec<_>>().join(", ");
            self.sys_log(&format!("Rolled a d20 for initiative: {line}."));
            if rolls.iter().filter(|&&v| v == max).count() == 1 {
                let w = rolls.iter().position(|&v| v == max).unwrap() as u8;
                self.stage = Stage::ChooseOrder { chooser: w };
                break;
            }
            self.sys_log("Tie. Rolling again.");
        }
        self.rng = r.0;
    }

    /// 2v2 / free-for-all: after turn order, everyone except the first player picks a battlefield.
    fn begin_battlefields(&mut self, db: &CardDb) {
        self.stage = Stage::ChooseBattlefield;
        let first = self.first;
        self.sys_log(&format!("{} goes first and does not bring a battlefield. Everyone else, choose a battlefield.", self.seats[first as usize].name));
        for s in 0..self.seats.len() {
            let opts = self.seats[s].bf_options.clone();
            if s as u8 != first && opts.len() == 1 && self.seats[s].chosen_bf.is_none() {
                self.seats[s].chosen_bf = Some(opts[0].clone());
                self.log_seat(s as u8, "Auto-selected the last remaining battlefield.");
            }
        }
        self.check_battlefields_done(db);
    }

    fn begin_mulligan(&mut self) {
        for s in 0..self.num_seats() {
            for _ in 0..OPENING_HAND {
                self.draw_one(s);
            }
        }
        self.stage = Stage::Mulligan;
        self.sys_log("Opening hands drawn. Mulligan up to 2 cards.");
    }

    fn start_play(&mut self) {
        self.stage = Stage::Play;
        self.turn = 1;
        self.active = self.first;
        self.phase = Phase::Awaken;
        self.sys_log("All mulligans are complete. Starting the game.");
        if self.options.auto_steps {
            self.run_start_of_turn();
        }
    }

    fn do_awaken(&mut self) {
        let a = self.active;
        let ids: Vec<CardId> = self.cards.iter().filter(|c| c.zone.is_board() && (c.zone.seat() == Some(a) || matches!(c.zone, Zone::BfCard(_)) && false)).map(|c| c.id).collect();
        for id in ids {
            self.cards[id as usize].exhausted = false;
        }
        self.phase = Phase::Awaken;
        self.log_seat(a, "Readied their cards.");
    }

    fn do_beginning(&mut self) {
        self.phase = Phase::Beginning;
        let a = self.active;
        let held: Vec<u8> = self.battlefields.iter().enumerate().filter(|(_, b)| b.controller == Some(a)).map(|(i, _)| i as u8).collect();
        for b in held {
            self.hold(a, b);
        }
    }

    fn do_channel(&mut self) {
        self.phase = Phase::Channel;
        let a = self.active;
        // The player who takes the last turn of the first round channels one extra rune.
        let n = if !self.seats[a as usize].had_turn && a == self.last_seat() && self.seats.len() > 1 { 3 } else { 2 };
        let got = self.channel(a, n);
        self.log_seat(a, &format!("Channeled {got} rune{}.", if got == 1 { "" } else { "s" }));
    }

    fn do_draw(&mut self) {
        self.phase = Phase::Draw;
        let a = self.active;
        if self.multi() && a == self.first && !self.seats[a as usize].had_turn {
            self.log_seat(a, "Skipped their first draw (first player).");
            return;
        }
        self.draw_one(a);
        self.log_seat(a, "Drew 1 card.");
    }

    fn run_start_of_turn(&mut self) {
        self.do_awaken();
        self.do_beginning();
        if self.winner.is_some() {
            return;
        }
        self.do_channel();
        self.do_draw();
        self.phase = Phase::Action;
        self.seats[self.active as usize].had_turn = true;
    }

    fn end_turn(&mut self) {
        // End phase: clear damage and temporary state on all units.
        self.phase = Phase::End;
        for c in self.cards.iter_mut() {
            if c.zone.is_unit_area() {
                c.damage = 0;
                c.stunned = false;
            }
        }
        self.showdown = None;
        let a = self.active;
        self.seats[a as usize].had_turn = true;
        self.log_seat(a, "Ended their turn.");
        let n = self.num_seats();
        let next = if self.seats[a as usize].extra_turns > 0 {
            self.seats[a as usize].extra_turns -= 1;
            self.log_seat(a, "Takes an extra turn.");
            a
        } else {
            self.next_seat(a)
        };
        let _ = n;
        self.active = next;
        self.turn += 1;
        self.phase = Phase::Awaken;
        if self.options.auto_steps {
            self.run_start_of_turn();
        }
    }

    fn hold(&mut self, seat: u8, bf: u8) {
        let turn = self.turn;
        let b = &mut self.battlefields[bf as usize];
        if b.scored[seat as usize] == turn && turn > 0 {
            return;
        }
        b.scored[seat as usize] = turn;
        self.add_score(seat, 1, &format!("Held battlefield {}", (b'A' + bf) as char));
    }

    fn conquer(&mut self, db: &CardDb, seat: u8, bf: u8) {
        let turn = self.turn;
        self.battlefields[bf as usize].controller = Some(seat);
        self.battlefields[bf as usize].contested_by = None;
        if self.battlefields[bf as usize].scored[seat as usize] == turn {
            self.log_seat(seat, &format!("Conquered battlefield {} (already scored there this turn).", (b'A' + bf) as char));
            return;
        }
        let target = self.options.win_points;
        let ally_holds = |b: &Battlefield| self.options.mode == Mode::Teams && b.controller.map(|c| c != seat && self.allied(c, seat)).unwrap_or(false);
        let scored_everywhere = self.battlefields.iter().enumerate().all(|(i, b)| i as u8 == bf || b.scored[seat as usize] == turn || ally_holds(b));
        self.battlefields[bf as usize].scored[seat as usize] = turn;
        if self.seats[seat as usize].score + 1 >= target && !scored_everywhere {
            // Final point rule: can't win by conquering unless every battlefield was scored this turn.
            self.draw_one(seat);
            self.log_seat(seat, &format!("Conquered battlefield {} but must draw instead of scoring the final point.", (b'A' + bf) as char));
            return;
        }
        let _ = db;
        self.add_score(seat, 1, &format!("Conquered battlefield {}", (b'A' + bf) as char));
    }

    /// Compute (energy, power, domains) cost of a card.
    fn cost(db: &CardDb, code: &str) -> (i32, i32, Vec<String>) {
        match db.get(code) {
            Some(d) => (d.energy.unwrap_or(0), d.power.unwrap_or(0), d.domains.clone()),
            None => (0, 0, vec![]),
        }
    }

    fn auto_pay(&mut self, db: &CardDb, seat: u8, card: CardId) -> R {
        let code = self.cards[card as usize].code.clone();
        let (energy, power, domains) = Self::cost(db, &code);
        let runes: Vec<CardId> = self.pile(Zone::Runes(seat)).to_vec();
        let dom_of = |g: &GameState, id: CardId| db.get(&g.cards[id as usize].code).map(|d| d.domains.clone()).unwrap_or_default();
        let matches = |g: &GameState, id: CardId| domains.is_empty() || domains.iter().any(|d| d == "Colorless") || dom_of(g, id).iter().any(|d| domains.contains(d));
        // Power: recycle matching runes, exhausted ones first so ready runes stay for energy.
        let mut recycle: Vec<CardId> = vec![];
        let mut cands: Vec<CardId> = runes.iter().copied().filter(|&r| matches(self, r)).collect();
        cands.sort_by_key(|&r| !self.cards[r as usize].exhausted);
        for r in cands {
            if recycle.len() as i32 >= power {
                break;
            }
            recycle.push(r);
        }
        if (recycle.len() as i32) < power {
            return Err(format!("Not enough matching runes to pay {power} power."));
        }
        // Energy: exhaust ready runes not being recycled, preferring non-matching ones.
        let mut ready: Vec<CardId> = runes.iter().copied().filter(|r| !recycle.contains(r) && !self.cards[*r as usize].exhausted).collect();
        ready.sort_by_key(|&r| matches(self, r));
        if (ready.len() as i32) < energy {
            return Err(format!("Not enough ready runes to pay {energy} energy."));
        }
        for &r in ready.iter().take(energy as usize) {
            self.cards[r as usize].exhausted = true;
        }
        for r in recycle {
            self.place(r, Zone::RuneDeck(seat), Pos::Bottom);
        }
        let parts: Vec<String> = [(energy, "energy"), (power, "power")].iter().filter(|(n, _)| *n > 0).map(|(n, w)| format!("{n} {w}")).collect();
        if !parts.is_empty() {
            self.log_seat(seat, &format!("Auto-paid {}.", parts.join(" and ")));
        }
        Ok(())
    }

    fn resolve_combat(&mut self, db: &CardDb, bf: u8) {
        let n = self.num_seats();
        let sides: Vec<Vec<CardId>> = (0..n).map(|s| self.pile(Zone::Bf(bf, s)).iter().copied().filter(|&id| !self.cards[id as usize].face_down).collect()).collect();
        let total: Vec<i32> = sides.iter().map(|v| v.iter().map(|&id| if self.cards[id as usize].stunned { 0 } else { self.might(db, id).max(0) }).sum()).collect();
        let mut lines = vec![];
        for s in 0..n as usize {
            // Damage from all other sides is assigned to this side's units, Tanks first, lethal before moving on.
            let incoming: i32 = (0..n as usize).filter(|&o| !self.allied(o as u8, s as u8)).map(|o| total[o]).sum();
            let mut order = sides[s].clone();
            order.sort_by_key(|&id| !db.get(&self.cards[id as usize].code).map(|d| d.text.contains("[Tank]")).unwrap_or(false));
            let mut left = incoming;
            for (k, &id) in order.iter().enumerate() {
                if left <= 0 {
                    break;
                }
                let need = (self.might(db, id) - self.cards[id as usize].damage).max(1);
                let give = if k == order.len() - 1 { left } else { need.min(left) };
                self.cards[id as usize].damage += give;
                left -= give;
            }
            lines.push(format!("{} {}", self.seats[s].name, total[s]));
        }
        self.sys_log(&format!("Combat at battlefield {}: {}.", (b'A' + bf) as char, lines.join(" vs ")));
        self.cleanup_lethal(db);
        self.settle(db, bf);
    }

    /// Seats with units at a battlefield.
    fn present(&self, bf: u8) -> Vec<u8> {
        (0..self.num_seats()).filter(|&s| !self.pile(Zone::Bf(bf, s)).is_empty()).collect()
    }

    /// Seats with face-up units, counted once per team.
    fn sides_at(&self, bf: u8) -> usize {
        let mut teams: Vec<u8> = (0..self.num_seats()).filter(|&s| self.pile(Zone::Bf(bf, s)).iter().any(|&id| !self.cards[id as usize].face_down)).map(|s| self.team(s)).collect();
        teams.sort();
        teams.dedup();
        teams.len()
    }

    /// After a showdown or combat: if only one side is left at the battlefield
    /// and it does not already control it, that side conquers. In 2v2 the
    /// conqueror is the teammate who moved in (or the active player).
    /// Returns whether anyone conquered.
    fn settle(&mut self, db: &CardDb, bf: u8) -> bool {
        let present = self.present(bf);
        let Some(&p0) = present.first() else { return false };
        if !present.iter().all(|&s| self.allied(s, p0)) {
            return false;
        }
        let ctrl = self.battlefields[bf as usize].controller;
        if ctrl.map(|c| present.contains(&c)).unwrap_or(false) {
            return false;
        }
        let by = self.battlefields[bf as usize].contested_by.filter(|c| present.contains(c));
        let who = by.or(present.iter().copied().find(|&s| s == self.active)).unwrap_or(p0);
        self.conquer(db, who, bf);
        true
    }

    fn cleanup_lethal(&mut self, db: &CardDb) {
        let dead: Vec<CardId> = self.cards.iter().filter(|c| c.zone.is_unit_area() && !c.face_down && c.damage > 0 && db.get(&c.code).map(|d| d.is_unit()).unwrap_or(false)).filter(|c| c.damage >= self.might(db, c.id)).map(|c| c.id).collect();
        for id in dead {
            let name = self.name_of(db, id);
            let owner = self.cards[id as usize].owner;
            self.place(id, Zone::Trash(owner), Pos::Top);
            self.sys_log(&format!("{name} was killed."));
        }
    }

    // ------------------------------------------------------------------ apply

    /// Apply an action on behalf of `actor`. In solo games, `actor` is the seat
    /// the controlling client is acting as.
    pub fn apply(&mut self, db: &CardDb, actor: u8, action: Action) -> R {
        self.check_seat(actor)?;
        if self.stage == Stage::Over && !matches!(action, Action::Ping { .. }) {
            return Err("The game is over.".into());
        }
        let r = self.apply_inner(db, actor, action);
        if r.is_ok() {
            self.version += 1;
        }
        r
    }

    fn apply_inner(&mut self, db: &CardDb, actor: u8, action: Action) -> R {
        use Action::*;
        let in_setup = !matches!(self.stage, Stage::Play);
        match &action {
            ChooseBattlefield { .. } | ChooseOrder { .. } | Mulligan { .. } | Concede | RollD20 | Ping { .. } => {}
            _ if in_setup => return Err("Finish the setup steps first.".into()),
            _ => {}
        }
        match action {
            ChooseBattlefield { code } => {
                if self.stage != Stage::ChooseBattlefield {
                    return Err("Battlefields are already locked.".into());
                }
                if self.multi() && actor == self.first {
                    return Err("The first player does not bring a battlefield.".into());
                }
                let p = &mut self.seats[actor as usize];
                if !p.bf_options.contains(&code) {
                    return Err("That battlefield is not one of your choices.".into());
                }
                p.chosen_bf = Some(code);
                self.log_seat(actor, "Locked in a battlefield.");
                self.check_battlefields_done(db);
            }
            ChooseOrder { go_first } => {
                let Stage::ChooseOrder { chooser } = self.stage else { return Err("It is not time to choose turn order.".into()) };
                if chooser != actor && !self.options.solo {
                    return Err("The other player chooses turn order.".into());
                }
                self.first = if go_first { chooser } else { self.next_seat(chooser) };
                let who = self.seats[self.first as usize].name.clone();
                self.log_seat(chooser, &format!("Chose to play {}. {} goes first.", if go_first { "first" } else { "second" }, who));
                if self.multi() {
                    self.begin_battlefields(db);
                } else {
                    self.begin_mulligan();
                }
            }
            Mulligan { cards } => {
                if self.stage != Stage::Mulligan {
                    return Err("The mulligan is not open.".into());
                }
                if self.seats[actor as usize].mulligan_done {
                    return Err("You already finished your mulligan.".into());
                }
                if cards.len() > MULLIGAN_MAX {
                    return Err("You may mulligan at most 2 cards.".into());
                }
                for &c in &cards {
                    if self.card(c).map(|x| x.zone) != Some(Zone::Hand(actor)) {
                        return Err("You can only mulligan cards from your hand.".into());
                    }
                }
                for &c in &cards {
                    self.place(c, Zone::Reveal(actor), Pos::Top);
                }
                for _ in 0..cards.len() {
                    self.draw_one(actor);
                }
                for &c in &cards {
                    self.place(c, Zone::Deck(actor), Pos::Bottom);
                }
                self.seats[actor as usize].mulligan_done = true;
                self.log_seat(actor, &format!("Finalized their mulligan ({} card{}).", cards.len(), if cards.len() == 1 { "" } else { "s" }));
                if self.seats.iter().all(|s| s.mulligan_done) {
                    self.start_play();
                }
            }
            StartTurn => {
                self.require_active(actor)?;
                self.run_start_of_turn();
            }
            AdvancePhase => {
                self.require_active(actor)?;
                match self.phase {
                    Phase::Awaken => self.do_beginning(),
                    Phase::Beginning => self.do_channel(),
                    Phase::Channel => self.do_draw(),
                    Phase::Draw => {
                        self.phase = Phase::Action;
                        self.log_seat(actor, "Ended beginning phase.");
                    }
                    Phase::Action => self.end_turn(),
                    Phase::End => self.do_awaken(),
                }
            }
            Awaken => {
                self.require_active(actor)?;
                self.do_awaken();
            }
            ScoreHolds => {
                self.require_active(actor)?;
                self.do_beginning();
            }
            Channel { n } => {
                let got = self.channel(actor, n.max(1));
                self.log_seat(actor, &format!("Channeled {got} rune{}.", if got == 1 { "" } else { "s" }));
            }
            Draw { n } => {
                for _ in 0..n.max(1) {
                    self.draw_one(actor);
                }
                self.log_seat(actor, &format!("Drew {} card{}.", n.max(1), if n > 1 { "s" } else { "" }));
            }
            EndTurn => {
                self.require_active(actor)?;
                self.end_turn();
            }
            TimeWarp => {
                self.seats[actor as usize].extra_turns += 1;
                self.log_seat(actor, "Queued an extra turn with Time Warp.");
            }
            Move { card, to, pos } => {
                self.may_touch(actor, card)?;
                self.own_zone(actor, to)?;
                self.valid_zone(to)?;
                let from = self.cards[card as usize].zone;
                let name = self.public_name(db, card, to);
                self.place(card, to, pos);
                // Standard move: a unit moving from base to a battlefield (or between battlefields) exhausts.
                if matches!(to, Zone::Bf(_, _)) && matches!(from, Zone::Base(_) | Zone::Bf(_, _)) {
                    self.cards[card as usize].exhausted = true;
                }
                if let Zone::Bf(b, s) = to {
                    let ctrl = self.battlefields.get(b as usize).and_then(|x| x.controller);
                    if !ctrl.map(|c| self.allied(c, s)).unwrap_or(false) && !self.cards[card as usize].face_down {
                        self.battlefields[b as usize].contested_by = Some(s);
                    }
                }
                let where_ = if to == Zone::Deck(self.cards[card as usize].owner) && pos == Pos::Bottom { "the bottom of their deck".to_string() } else { format!("their {}", to.label()) };
                self.log_seat(actor, &format!("Moved {name} from {} to {where_}.", from.label()));
            }
            PlayCard { card, to, autopay } => {
                self.may_touch(actor, card)?;
                let c = self.cards[card as usize].clone();
                if !matches!(c.zone, Zone::Hand(_) | Zone::Champion(_) | Zone::Reveal(_) | Zone::Bf(_, _) | Zone::Trash(_)) {
                    return Err("That card can't be played from there.".into());
                }
                let def = db.get(&c.code).ok_or("Unknown card.")?;
                if autopay {
                    self.auto_pay(db, actor, card)?;
                }
                let dest = to.unwrap_or(if def.is_spell() { Zone::Chain } else { Zone::Base(actor) });
                self.valid_zone(dest)?;
                self.place(card, dest, Pos::Top);
                if def.is_unit() && dest.is_unit_area() {
                    let accel = false;
                    self.cards[card as usize].exhausted = !accel;
                }
                self.log_seat(actor, &format!("Played {}.", def.name));
            }
            AutoPay { card } => {
                self.may_touch(actor, card)?;
                self.auto_pay(db, actor, card)?;
            }
            Hide { card, bf } => {
                self.may_touch(actor, card)?;
                if bf as usize >= self.battlefields.len() {
                    return Err("Unknown battlefield.".into());
                }
                self.place(card, Zone::Bf(bf, actor), Pos::Top);
                self.cards[card as usize].face_down = true;
                self.log_seat(actor, &format!("Hid a card at battlefield {}.", (b'A' + bf) as char));
            }
            SetExhausted { card, exhausted } => {
                self.may_touch(actor, card)?;
                self.card_mut(card)?.exhausted = exhausted;
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("{} {n}.", if exhausted { "Exhausted" } else { "Readied" }));
            }
            ToggleExhausted { card } => {
                self.may_touch(actor, card)?;
                let e = !self.card(card).ok_or("missing")?.exhausted;
                self.card_mut(card)?.exhausted = e;
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("{} {n}.", if e { "Exhausted" } else { "Readied" }));
            }
            Flip { card } => {
                self.may_touch(actor, card)?;
                let c = self.card_mut(card)?;
                c.face_down = !c.face_down;
                let up = !c.face_down;
                if up {
                    let n = self.name_of(db, card);
                    self.log_seat(actor, &format!("Revealed hidden card {n}."));
                } else {
                    self.log_seat(actor, "Turned a card face down.");
                }
            }
            Damage { card, delta } => {
                let c = self.card_mut(card)?;
                c.damage = (c.damage + delta).max(0);
                let d = c.damage;
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("Set damage on {n} to {d}."));
            }
            Buff { card, delta } => {
                let c = self.card_mut(card)?;
                c.buffs = (c.buffs + delta).max(0);
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("{} {n}.", if delta > 0 { "Buffed" } else { "Removed a buff from" }));
            }
            Counter { card, delta } => {
                let c = self.card_mut(card)?;
                c.counters += delta;
                let v = c.counters;
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("Set counters on {n} to {v}."));
            }
            Stun { card } => {
                let c = self.card_mut(card)?;
                c.stunned = !c.stunned;
                let s = c.stunned;
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("{} {n}.", if s { "Stunned" } else { "Removed stun from" }));
            }
            Kill { card } => {
                self.may_touch(actor, card)?;
                let owner = self.cards[card as usize].owner;
                let n = self.name_of(db, card);
                self.place(card, Zone::Trash(owner), Pos::Top);
                self.log_seat(actor, &format!("Killed {n}."));
            }
            Recall { card } => {
                self.may_touch(actor, card)?;
                let owner = self.cards[card as usize].owner;
                self.place(card, Zone::Base(owner), Pos::Top);
                let n = self.name_of(db, card);
                self.log_seat(actor, &format!("Recalled {n} to base."));
            }
            Banish { card } => {
                self.may_touch(actor, card)?;
                let owner = self.cards[card as usize].owner;
                let n = self.name_of(db, card);
                self.place(card, Zone::Banish(owner), Pos::Top);
                self.log_seat(actor, &format!("Banished {n}."));
            }
            Reveal { card } => {
                self.may_touch(actor, card)?;
                let c = self.card_mut(card)?;
                c.shown = !c.shown;
                let s = c.shown;
                if s {
                    let n = self.name_of(db, card);
                    self.log_seat(actor, &format!("Revealed {n} from their hand."));
                } else {
                    self.log_seat(actor, "Hid a card in their hand.");
                }
            }
            RevealHand { shown } => {
                self.seats[actor as usize].hand_shown = shown;
                self.log_seat(actor, if shown { "Revealed their hand." } else { "Hid their hand." });
            }
            CreateToken { code, to, count } => {
                let def = db.get(&code).ok_or("Unknown token.")?;
                self.valid_zone(to)?;
                let owner = to.seat().unwrap_or(actor);
                for _ in 0..count.clamp(1, 20) {
                    let id = self.spawn(&def.code, owner, to, true);
                    if def.is_unit() {
                        self.cards[id as usize].exhausted = true;
                    }
                }
                self.log_seat(actor, &format!("Created {} {} token{}.", count.max(1), def.name, if count > 1 { "s" } else { "" }));
            }
            Ping { card } => {
                self.card(card).ok_or("missing")?;
                let nonce = self.ping.as_ref().map(|p| p.nonce + 1).unwrap_or(1);
                self.ping = Some(crate::game::Ping { card, seat: actor, nonce });
            }
            Shuffle { zone } => {
                self.own_zone(actor, zone)?;
                self.shuffle_zone(zone);
                self.log_seat(actor, &format!("Shuffled their {}.", zone.label()));
            }
            Peek { zone, n, bottom } => {
                self.own_zone(actor, zone)?;
                let p = self.pile(zone);
                let n = (n as usize).min(p.len());
                let cards: Vec<CardId> = if bottom { p[..n].to_vec() } else { p[p.len() - n..].iter().rev().copied().collect() };
                self.peeks.retain(|x| x.seat != actor);
                self.peeks.push(crate::game::Peek { seat: actor, cards });
                self.log_seat(actor, &format!("Looked at the {} {n} card{} of their {}.", if bottom { "bottom" } else { "top" }, if n == 1 { "" } else { "s" }, zone.label()));
            }
            Search { zone } => {
                self.own_zone(actor, zone)?;
                let cards: Vec<CardId> = self.pile(zone).iter().rev().copied().collect();
                self.peeks.retain(|x| x.seat != actor);
                self.peeks.push(crate::game::Peek { seat: actor, cards });
                self.log_seat(actor, &format!("Searched their {}.", zone.label()));
            }
            ClosePeek => {
                self.peeks.retain(|x| x.seat != actor);
                self.log_seat(actor, "Closed deck peek.");
            }
            Burn { n } => {
                for _ in 0..n.max(1) {
                    if let Some(&top) = self.pile(Zone::Deck(actor)).last() {
                        self.place(top, Zone::Trash(actor), Pos::Top);
                    }
                }
                self.log_seat(actor, &format!("Burned {} card{}.", n.max(1), if n > 1 { "s" } else { "" }));
            }
            RevealTop { zone } => {
                self.own_zone(actor, zone)?;
                let top = *self.pile(zone).last().ok_or("That deck is empty.")?;
                self.place(top, Zone::Reveal(actor), Pos::Top);
                let n = self.name_of(db, top);
                self.log_seat(actor, &format!("Revealed {n} from the top of their {}.", zone.label()));
            }
            RecycleRune { card } => {
                self.may_touch(actor, card)?;
                let owner = self.cards[card as usize].owner;
                if self.cards[card as usize].zone != Zone::Runes(owner) {
                    return Err("Only channeled runes can be recycled.".into());
                }
                self.place(card, Zone::RuneDeck(owner), Pos::Bottom);
                self.log_seat(actor, "Recycled a rune to the bottom of their rune deck.");
            }
            ReadyRunes => {
                let ids: Vec<CardId> = self.pile(Zone::Runes(actor)).to_vec();
                if ids.iter().all(|&i| !self.cards[i as usize].exhausted) {
                    return Err("No exhausted runes to ready.".into());
                }
                for i in ids {
                    self.cards[i as usize].exhausted = false;
                }
                self.log_seat(actor, "Readied all their runes.");
            }
            Score { seat, delta } => {
                self.check_seat(seat)?;
                self.add_score(seat, delta, if delta >= 0 { "Raised their score" } else { "Lowered their score" });
            }
            Conquer { bf } => {
                self.bf_check(bf)?;
                self.conquer(db, actor, bf);
            }
            Hold { bf } => {
                self.bf_check(bf)?;
                self.hold(actor, bf);
            }
            SetController { bf, seat } => {
                self.bf_check(bf)?;
                if let Some(s) = seat {
                    self.check_seat(s)?;
                }
                self.battlefields[bf as usize].controller = seat;
                self.log_seat(actor, &format!("Set the controller of battlefield {}.", (b'A' + bf) as char));
            }
            StartShowdown { bf } => {
                self.bf_check(bf)?;
                let sides = self.sides_at(bf);
                self.showdown = Some(crate::game::Showdown { bf, focus: actor, combat: sides >= 2, passes: 0 });
                self.log_seat(actor, &format!("Started a {} at battlefield {}.", if sides >= 2 { "combat showdown" } else { "showdown" }, (b'A' + bf) as char));
            }
            PassFocus => {
                let focus = self.showdown.as_ref().ok_or("There is no showdown.")?.focus;
                let next = self.next_seat(focus);
                let sd = self.showdown.as_mut().unwrap();
                sd.focus = next;
                sd.passes += 1;
                self.log_seat(actor, "Passed focus.");
            }
            EndShowdown => {
                let sd = self.showdown.take().ok_or("There is no showdown.")?;
                if sd.combat {
                    self.resolve_combat(db, sd.bf);
                } else if !self.settle(db, sd.bf) {
                    self.log_seat(actor, "Ended the showdown.");
                }
            }
            ResolveCombat { bf } => {
                self.bf_check(bf)?;
                self.resolve_combat(db, bf);
            }
            CleanupLethal => self.cleanup_lethal(db),
            ApproveChain => {
                let top = *self.pile(Zone::Chain).last().ok_or("The chain is empty.")?;
                let owner = self.cards[top as usize].owner;
                let n = self.name_of(db, top);
                self.place(top, Zone::Trash(owner), Pos::Top);
                self.log_seat(actor, &format!("Resolved {n} from the chain."));
            }
            RollD20 => {
                let mut r = self.rng();
                let v = r.below(20) as u32 + 1;
                self.rng = r.0;
                self.last_roll = Some((actor, v));
                self.log_seat(actor, &format!("Rolled a d20: {v}."));
            }
            Concede => {
                if self.stage == Stage::Over {
                    return Err("The game is over.".into());
                }
                let n = self.num_seats();
                // A 2v2 concession forfeits for the whole team.
                for s in 0..n {
                    if self.allied(s, actor) {
                        self.seats[s as usize].conceded = true;
                    }
                }
                self.log_seat(actor, if self.options.mode == Mode::Teams && n > 2 { "Conceded for their team." } else { "Conceded." });
                if n == 1 {
                    self.stage = Stage::Over;
                } else {
                    let remaining: Vec<u8> = (0..n).filter(|&s| !self.seats[s as usize].conceded).collect();
                    let teams_left = remaining.iter().filter(|&&s| remaining.iter().all(|&o| o >= s || !self.allied(o, s))).count();
                    if teams_left == 1 {
                        self.finish(remaining[0], if n > 2 { "every opponent conceded" } else { "opponent conceded" });
                    } else {
                        self.after_free_for_all_concede(db, actor);
                    }
                }
            }
        }
        Ok(())
    }

    /// A free-for-all goes on after one player concedes: their setup choices
    /// are filled in, their turn and showdown focus pass on.
    fn after_free_for_all_concede(&mut self, db: &CardDb, seat: u8) {
        let p = &mut self.seats[seat as usize];
        p.mulligan_done = true;
        p.extra_turns = 0;
        if p.chosen_bf.is_none() {
            p.bf_options.clear();
        }
        match self.stage {
            Stage::ChooseOrder { chooser } if chooser == seat => {
                self.first = self.next_seat(seat);
                self.sys_log(&format!("{} goes first.", self.seats[self.first as usize].name));
                self.begin_battlefields(db);
            }
            Stage::ChooseBattlefield => self.check_battlefields_done(db),
            Stage::Mulligan => {
                if self.seats.iter().all(|s| s.mulligan_done) {
                    self.start_play();
                }
            }
            Stage::Play => {
                if self.showdown.as_ref().map(|sd| sd.focus == seat).unwrap_or(false) {
                    let next = self.next_seat(seat);
                    self.showdown.as_mut().unwrap().focus = next;
                }
                if self.active == seat {
                    self.end_turn();
                }
            }
            _ => {}
        }
    }

    fn require_active(&self, actor: u8) -> R {
        if self.options.solo || actor == self.active {
            Ok(())
        } else {
            Err("It is not your turn.".into())
        }
    }

    fn bf_check(&self, bf: u8) -> R {
        if (bf as usize) < self.battlefields.len() {
            Ok(())
        } else {
            Err("Unknown battlefield.".into())
        }
    }

    fn valid_zone(&self, z: Zone) -> R {
        if let Some(s) = z.seat() {
            self.check_seat(s)?;
        }
        match z {
            Zone::Bf(b, _) | Zone::BfCard(b) => self.bf_check(b),
            Zone::Void => Err("Invalid destination.".into()),
            _ => Ok(()),
        }
    }

    /// Name for logging a move without leaking hidden information.
    fn public_name(&self, db: &CardDb, id: CardId, to: Zone) -> String {
        let c = &self.cards[id as usize];
        if (c.zone.is_hidden() && to.is_hidden()) || c.face_down {
            "a card".into()
        } else {
            self.name_of(db, id)
        }
    }

    // ------------------------------------------------------------------ views

    /// Can `viewer` see the face of card `c`? `None` = spectator.
    pub fn visible(&self, c: &CardInst, viewer: Option<u8>, spectators_see_hands: bool) -> bool {
        if self.options.solo && viewer.is_some() {
            return !matches!(c.zone, Zone::Deck(_) | Zone::RuneDeck(_)) || self.peeks.iter().any(|p| p.cards.contains(&c.id));
        }
        if self.stage == Stage::Over && !matches!(c.zone, Zone::Deck(_) | Zone::RuneDeck(_)) {
            return true;
        }
        // 2v2 teammates share information: hands and face-down cards.
        let ally_view = |s: u8| viewer.map(|v| v == s || (self.options.mode == Mode::Teams && self.allied(v, s))).unwrap_or(false);
        let owner_view = ally_view(c.owner);
        match c.zone {
            Zone::Hand(s) => ally_view(s) || c.shown || self.seats[s as usize].hand_shown || (viewer.is_none() && spectators_see_hands),
            Zone::Deck(_) | Zone::RuneDeck(_) => c.shown || self.peeks.iter().any(|p| Some(p.seat) == viewer && p.cards.contains(&c.id)),
            Zone::Side(s) => viewer == Some(s),
            _ if c.face_down => owner_view || (viewer.is_none() && spectators_see_hands),
            _ => true,
        }
    }

    /// Redacted copy of the state for a viewer.
    pub fn view_for(&self, viewer: Option<u8>, spectators_see_hands: bool) -> GameState {
        let mut v = self.clone();
        for c in v.cards.iter_mut() {
            if !self.visible(c, viewer, spectators_see_hands) {
                c.code.clear();
            }
        }
        if !(self.options.solo && viewer.is_some()) {
            v.peeks.retain(|p| Some(p.seat) == viewer);
        }
        // Void cards are irrelevant; RNG state would let clients predict shuffles.
        v.rng = 0;
        v
    }

    pub fn hand(&self, seat: u8) -> &[CardId] {
        self.pile(Zone::Hand(seat))
    }
}

/// "Channeled 3 runes." -> Some(3) for (Channeled, rune).
fn count_of(text: &str, verb: &str, noun: &str) -> Option<u32> {
    let rest = text.strip_prefix(verb)?.strip_prefix(' ')?;
    let (n, tail) = rest.split_once(' ')?;
    let n: u32 = n.parse().ok()?;
    (tail == format!("{noun}.") || tail == format!("{noun}s.")).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::tests::db;
    use crate::decklist::starter_deck;

    #[test]
    fn log_merges_repeats() {
        let db = db();
        let mut g = into_play(&db);
        let a = g.active;
        for _ in 0..4 {
            g.apply(&db, a, Action::Channel { n: 1 }).unwrap();
        }
        assert_eq!(g.log.last().unwrap().text, "Channeled 4 runes.");
        assert_eq!(count_of("Drew 1 card.", "Drew", "card"), Some(1));
        assert_eq!(count_of("Drew a card.", "Drew", "card"), None);
    }

    pub fn two_player(db: &CardDb) -> GameState {
        let a = starter_deck(db, "SFD-181").unwrap();
        let b = starter_deck(db, "SFD-205").unwrap();
        let seats = vec![
            NewSeat { name: "Alice".into(), bf_options: a.battlefields.clone(), deck: a },
            NewSeat { name: "Bob".into(), bf_options: b.battlefields.clone(), deck: b },
        ];
        GameState::new(db, seats, GameOptions::default(), 42, None)
    }

    pub fn into_play(db: &CardDb) -> GameState {
        let mut g = two_player(db);
        let b0 = g.seats[0].bf_options[0].clone();
        let b1 = g.seats[1].bf_options[1].clone();
        g.apply(db, 0, Action::ChooseBattlefield { code: b0 }).unwrap();
        g.apply(db, 1, Action::ChooseBattlefield { code: b1 }).unwrap();
        let Stage::ChooseOrder { chooser } = g.stage else { panic!("{:?}", g.stage) };
        g.apply(db, chooser, Action::ChooseOrder { go_first: true }).unwrap();
        g.apply(db, 0, Action::Mulligan { cards: vec![] }).unwrap();
        g.apply(db, 1, Action::Mulligan { cards: vec![] }).unwrap();
        g
    }

    #[test]
    fn setup_flow() {
        let db = db();
        let mut g = two_player(&db);
        assert_eq!(g.stage, Stage::ChooseBattlefield);
        assert_eq!(g.pile(Zone::Deck(0)).len(), 39);
        assert_eq!(g.pile(Zone::RuneDeck(0)).len(), 12);
        assert!(g.apply(&db, 0, Action::Draw { n: 1 }).is_err(), "no play during setup");
        assert!(g.apply(&db, 0, Action::ChooseBattlefield { code: "OGN-001".into() }).is_err());
        let b0 = g.seats[0].bf_options[0].clone();
        g.apply(&db, 0, Action::ChooseBattlefield { code: b0 }).unwrap();
        let b1 = g.seats[1].bf_options[0].clone();
        g.apply(&db, 1, Action::ChooseBattlefield { code: b1 }).unwrap();
        assert_eq!(g.battlefields.len(), 2);
        let Stage::ChooseOrder { chooser } = g.stage else { panic!() };
        let other = 1 - chooser;
        assert!(g.apply(&db, other, Action::ChooseOrder { go_first: true }).is_err());
        g.apply(&db, chooser, Action::ChooseOrder { go_first: false }).unwrap();
        assert_eq!(g.first, other);
        assert_eq!(g.stage, Stage::Mulligan);
        assert_eq!(g.hand(0).len(), 4);
        let mull: Vec<CardId> = g.hand(0)[..2].to_vec();
        g.apply(&db, 0, Action::Mulligan { cards: mull.clone() }).unwrap();
        assert_eq!(g.hand(0).len(), 4);
        assert_eq!(&g.pile(Zone::Deck(0))[..2], &[mull[1], mull[0]]);
        assert!(g.apply(&db, 0, Action::Mulligan { cards: vec![] }).is_err());
        g.apply(&db, 1, Action::Mulligan { cards: vec![] }).unwrap();
        assert_eq!(g.stage, Stage::Play);
        // auto steps: first player channeled 2 and drew 1
        assert_eq!(g.active, other);
        assert_eq!(g.pile(Zone::Runes(other)).len(), 2);
        assert_eq!(g.hand(other).len(), 5);
        assert_eq!(g.phase, Phase::Action);
    }

    #[test]
    fn second_player_channels_three_and_turns_pass() {
        let db = db();
        let mut g = into_play(&db);
        let first = g.active;
        let second = 1 - first;
        assert!(g.apply(&db, second, Action::EndTurn).is_err());
        g.apply(&db, first, Action::EndTurn).unwrap();
        assert_eq!(g.active, second);
        assert_eq!(g.pile(Zone::Runes(second)).len(), 3);
        g.apply(&db, second, Action::EndTurn).unwrap();
        assert_eq!(g.pile(Zone::Runes(first)).len(), 4);
        assert_eq!(g.turn, 3);
    }

    #[test]
    fn play_with_autopay_and_move_and_combat() {
        let db = db();
        let mut g = into_play(&db);
        let a = g.active;
        // give plenty of runes
        g.apply(&db, a, Action::Channel { n: 8 }).unwrap();
        let unit = g.hand(a).iter().copied().find(|&id| db.get(&g.cards[id as usize].code).map(|d| d.is_unit()).unwrap_or(false));
        let unit = match unit {
            Some(u) => u,
            None => {
                let id = g.pile(Zone::Deck(a)).iter().copied().find(|&id| db.get(&g.cards[id as usize].code).unwrap().is_unit()).unwrap();
                g.apply(&db, a, Action::Move { card: id, to: Zone::Hand(a), pos: Pos::Top }).unwrap();
                id
            }
        };
        let (e, p, _) = GameState::cost(&db, &g.cards[unit as usize].code.clone());
        let runes_before = g.pile(Zone::Runes(a)).len();
        g.apply(&db, a, Action::PlayCard { card: unit, to: None, autopay: true }).unwrap();
        assert_eq!(g.cards[unit as usize].zone, Zone::Base(a));
        assert!(g.cards[unit as usize].exhausted);
        assert_eq!(g.pile(Zone::Runes(a)).len(), runes_before - p as usize);
        let exhausted = g.pile(Zone::Runes(a)).iter().filter(|&&r| g.cards[r as usize].exhausted).count();
        assert_eq!(exhausted, e as usize);
        // Move to battlefield 0 (uncontrolled, empty) and conquer through a showdown.
        g.apply(&db, a, Action::SetExhausted { card: unit, exhausted: false }).unwrap();
        g.apply(&db, a, Action::Move { card: unit, to: Zone::Bf(0, a), pos: Pos::Top }).unwrap();
        assert!(g.cards[unit as usize].exhausted);
        g.apply(&db, a, Action::StartShowdown { bf: 0 }).unwrap();
        g.apply(&db, a, Action::PassFocus).unwrap();
        g.apply(&db, a, Action::EndShowdown).unwrap();
        assert_eq!(g.battlefields[0].controller, Some(a));
        assert_eq!(g.seats[a as usize].score, 1);
        // conquering again this turn doesn't score twice
        g.apply(&db, a, Action::Conquer { bf: 0 }).unwrap();
        assert_eq!(g.seats[a as usize].score, 1);
        // Opponent unit arrives: combat kills the weaker side.
        let o = 1 - a;
        let tok = db.tokens().find(|t| t.is_unit()).unwrap().code.clone();
        g.apply(&db, o, Action::CreateToken { code: tok, to: Zone::Bf(0, o), count: 1 }).unwrap();
        let before = g.pile(Zone::Bf(0, o)).len();
        g.apply(&db, a, Action::ResolveCombat { bf: 0 }).unwrap();
        assert!(g.pile(Zone::Bf(0, o)).len() <= before);
        // Next turn: holding scores at the beginning phase.
        g.apply(&db, a, Action::EndTurn).unwrap();
        g.apply(&db, o, Action::EndTurn).unwrap();
        if g.battlefields[0].controller == Some(a) {
            assert_eq!(g.seats[a as usize].score, 2);
        }
    }

    #[test]
    fn hidden_info_is_redacted() {
        let db = db();
        let g = into_play(&db);
        let v = g.view_for(Some(0), false);
        for &id in v.hand(1) {
            assert!(v.cards[id as usize].code.is_empty());
        }
        for &id in v.hand(0) {
            assert!(!v.cards[id as usize].code.is_empty());
        }
        for &id in v.pile(Zone::Deck(0)) {
            assert!(v.cards[id as usize].code.is_empty());
        }
        assert_eq!(v.rng, 0);
        let spec = g.view_for(None, false);
        assert!(spec.hand(0).iter().all(|&id| spec.cards[id as usize].code.is_empty()));
        let spec2 = g.view_for(None, true);
        assert!(spec2.hand(0).iter().all(|&id| !spec2.cards[id as usize].code.is_empty()));
        // revealing a hand shows it
        let mut g2 = g.clone();
        g2.apply(&db, 1, Action::RevealHand { shown: true }).unwrap();
        let v = g2.view_for(Some(0), false);
        assert!(v.hand(1).iter().all(|&id| !v.cards[id as usize].code.is_empty()));
    }

    #[test]
    fn permissions() {
        let db = db();
        let mut g = into_play(&db);
        let opp_hand = g.hand(1)[0];
        assert!(g.apply(&db, 0, Action::Move { card: opp_hand, to: Zone::Trash(1), pos: Pos::Top }).is_err());
        assert!(g.apply(&db, 0, Action::Peek { zone: Zone::Deck(1), n: 2, bottom: false }).is_err());
        assert!(g.apply(&db, 0, Action::Move { card: g.hand(0)[0], to: Zone::Hand(1), pos: Pos::Top }).is_err());
    }

    #[test]
    fn peek_search_shuffle() {
        let db = db();
        let mut g = into_play(&db);
        g.apply(&db, 0, Action::Peek { zone: Zone::Deck(0), n: 3, bottom: false }).unwrap();
        let v = g.view_for(Some(0), false);
        assert_eq!(v.peeks.len(), 1);
        assert_eq!(v.peeks[0].cards.len(), 3);
        assert!(v.peeks[0].cards.iter().all(|&id| !v.cards[id as usize].code.is_empty()));
        assert!(g.view_for(Some(1), false).peeks.is_empty());
        let top = v.peeks[0].cards[0];
        g.apply(&db, 0, Action::Move { card: top, to: Zone::Deck(0), pos: Pos::Bottom }).unwrap();
        assert_eq!(g.pile(Zone::Deck(0))[0], top);
        g.apply(&db, 0, Action::ClosePeek).unwrap();
        let before: Vec<String> = g.pile(Zone::Deck(0)).iter().map(|&i| g.cards[i as usize].code.clone()).collect();
        g.apply(&db, 0, Action::Shuffle { zone: Zone::Deck(0) }).unwrap();
        let after: Vec<String> = g.pile(Zone::Deck(0)).iter().map(|&i| g.cards[i as usize].code.clone()).collect();
        assert_ne!(before, after);
        let (mut b, mut a) = (before.clone(), after.clone());
        b.sort();
        a.sort();
        assert_eq!(a, b);
    }

    #[test]
    fn burn_out_scores_for_opponent() {
        let db = db();
        let mut g = into_play(&db);
        let ids: Vec<CardId> = g.pile(Zone::Deck(0)).to_vec();
        for id in ids {
            g.apply(&db, 0, Action::Move { card: id, to: Zone::Trash(0), pos: Pos::Top }).unwrap();
        }
        g.apply(&db, 0, Action::Draw { n: 1 }).unwrap();
        assert_eq!(g.seats[1].score, 1);
        assert!(g.pile(Zone::Trash(0)).is_empty());
    }

    #[test]
    fn winning_and_concede() {
        let db = db();
        let mut g = into_play(&db);
        g.apply(&db, 0, Action::Score { seat: 0, delta: 7 }).unwrap();
        assert!(g.winner.is_none());
        g.apply(&db, 0, Action::Score { seat: 0, delta: 1 }).unwrap();
        assert_eq!(g.winner, Some(0));
        assert!(g.apply(&db, 0, Action::Draw { n: 1 }).is_err());
        let mut g = into_play(&db);
        g.apply(&db, 1, Action::Concede).unwrap();
        assert_eq!(g.winner, Some(0));
    }

    #[test]
    fn final_point_needs_all_battlefields() {
        let db = db();
        let mut g = into_play(&db);
        let a = g.active;
        g.apply(&db, a, Action::Score { seat: a, delta: 7 }).unwrap();
        let hand = g.hand(a).len();
        g.apply(&db, a, Action::Conquer { bf: 0 }).unwrap();
        assert_eq!(g.seats[a as usize].score, 7);
        assert_eq!(g.hand(a).len(), hand + 1);
        g.apply(&db, a, Action::Conquer { bf: 1 }).unwrap();
        // bf0 already scored this turn -> bf1 completes the set
        assert_eq!(g.winner, Some(a));
    }

    #[test]
    fn tokens_vanish_and_chain_resolves() {
        let db = db();
        let mut g = into_play(&db);
        let tok = db.tokens().next().unwrap().code.clone();
        g.apply(&db, 0, Action::CreateToken { code: tok, to: Zone::Base(0), count: 2 }).unwrap();
        let t = *g.pile(Zone::Base(0)).last().unwrap();
        g.apply(&db, 0, Action::Kill { card: t }).unwrap();
        assert_eq!(g.cards[t as usize].zone, Zone::Void);
        assert!(!g.pile(Zone::Trash(0)).contains(&t));
        let spell = g.cards.iter().find(|c| c.owner == 0 && matches!(c.zone, Zone::Deck(_)) && db.get(&c.code).unwrap().is_spell()).map(|c| c.id);
        if let Some(s) = spell {
            g.apply(&db, 0, Action::Move { card: s, to: Zone::Hand(0), pos: Pos::Top }).unwrap();
            g.apply(&db, 0, Action::PlayCard { card: s, to: None, autopay: false }).unwrap();
            assert_eq!(g.cards[s as usize].zone, Zone::Chain);
            g.apply(&db, 1, Action::ApproveChain).unwrap();
            assert_eq!(g.cards[s as usize].zone, Zone::Trash(0));
        }
    }

    #[test]
    fn goldfish_single_seat() {
        let db = db();
        let a = starter_deck(&db, "SFD-181").unwrap();
        let mut g = GameState::new(&db, vec![NewSeat { name: "Solo".into(), bf_options: a.battlefields.clone(), deck: a }], GameOptions { solo: true, ..Default::default() }, 1, None);
        let b = g.seats[0].bf_options[0].clone();
        g.apply(&db, 0, Action::ChooseBattlefield { code: b }).unwrap();
        assert_eq!(g.battlefields.len(), 2);
        g.apply(&db, 0, Action::ChooseOrder { go_first: true }).unwrap();
        g.apply(&db, 0, Action::Mulligan { cards: vec![] }).unwrap();
        assert_eq!(g.stage, Stage::Play);
        g.apply(&db, 0, Action::EndTurn).unwrap();
        assert_eq!(g.active, 0);
        assert_eq!(g.turn, 2);
    }

    // ------------------------------------------------------------ 2v2 / free-for-all

    pub fn four_player(db: &CardDb, mode: Mode) -> GameState {
        let names = ["Ann", "Bo", "Cy", "Di"];
        let seats = ["SFD-181", "SFD-205", "UNL-193", "UNL-187"]
            .iter()
            .zip(names)
            .map(|(l, n)| {
                let d = starter_deck(db, l).unwrap();
                NewSeat { name: n.into(), bf_options: d.battlefields.clone(), deck: d }
            })
            .collect();
        GameState::new(db, seats, GameOptions { mode, win_points: mode.default_points(), ..Default::default() }, 7, None)
    }

    /// Roll, choose to go first, pick battlefields, keep hands.
    pub fn four_into_play(db: &CardDb, mode: Mode) -> GameState {
        let mut g = four_player(db, mode);
        let Stage::ChooseOrder { chooser } = g.stage else { panic!("{:?}", g.stage) };
        g.apply(db, chooser, Action::ChooseOrder { go_first: true }).unwrap();
        for s in 0..4u8 {
            if s != g.first {
                let b = g.seats[s as usize].bf_options[0].clone();
                g.apply(db, s, Action::ChooseBattlefield { code: b }).unwrap();
            }
        }
        for s in 0..4u8 {
            g.apply(db, s, Action::Mulligan { cards: vec![] }).unwrap();
        }
        g
    }

    fn unit(db: &CardDb, g: &GameState) -> String {
        g.cards.iter().filter_map(|c| db.get(&c.code)).find(|d| d.is_unit() && d.might.unwrap_or(0) >= 2 && !d.text.contains("[Tank]")).unwrap().code.clone()
    }

    #[test]
    fn multi_setup_order_battlefields_draw_and_runes() {
        let db = db();
        for mode in [Mode::Teams, Mode::FreeForAll] {
            let mut g = four_player(&db, mode);
            // Turn order is decided before battlefields.
            let Stage::ChooseOrder { chooser } = g.stage else { panic!("{:?}", g.stage) };
            assert_eq!(g.seats.iter().filter(|s| s.roll.is_some()).count(), 4);
            g.apply(&db, chooser, Action::ChooseOrder { go_first: false }).unwrap();
            let first = g.first;
            assert_eq!(first, (chooser + 1) % 4);
            assert_eq!(g.stage, Stage::ChooseBattlefield);
            let b = g.seats[first as usize].bf_options[0].clone();
            assert!(g.apply(&db, first, Action::ChooseBattlefield { code: b }).is_err(), "first player brings no battlefield");
            for k in 1..4u8 {
                let s = (first + k) % 4;
                let b = g.seats[s as usize].bf_options[0].clone();
                g.apply(&db, s, Action::ChooseBattlefield { code: b }).unwrap();
            }
            assert_eq!(g.battlefields.len(), 3);
            assert_eq!(g.cards[g.battlefields[0].card as usize].owner, (first + 1) % 4);
            assert_eq!(g.stage, Stage::Mulligan);
            for s in 0..4u8 {
                g.apply(&db, s, Action::Mulligan { cards: vec![] }).unwrap();
            }
            assert_eq!(g.stage, Stage::Play);
            assert_eq!(g.active, first);
            assert_eq!(g.hand(first).len(), 4, "first player skips the first draw");
            assert_eq!(g.pile(Zone::Runes(first)).len(), 2);
            let mut order = vec![first];
            for _ in 0..3 {
                let a = g.active;
                g.apply(&db, a, Action::EndTurn).unwrap();
                order.push(g.active);
                assert_eq!(g.hand(g.active).len(), 5);
            }
            assert_eq!(order, (0..4).map(|k| (first + k) % 4).collect::<Vec<_>>());
            let last = g.active;
            assert_eq!(last, g.last_seat());
            assert_eq!(g.pile(Zone::Runes(last)).len(), 3, "last player channels an extra rune");
            assert_eq!(g.pile(Zone::Runes((first + 1) % 4)).len(), 2);
            g.apply(&db, last, Action::EndTurn).unwrap();
            assert_eq!(g.hand(first).len(), 5, "first player draws from their second turn");
        }
    }

    #[test]
    fn teams_share_score_and_win_at_eleven() {
        let db = db();
        let mut g = four_into_play(&db, Mode::Teams);
        assert_eq!(g.options.win_points, 11);
        assert!(g.allied(0, 2) && g.allied(1, 3) && !g.allied(0, 1));
        g.apply(&db, 0, Action::Score { seat: 0, delta: 4 }).unwrap();
        assert_eq!(g.seats[2].score, 4);
        g.apply(&db, 2, Action::Score { seat: 2, delta: 6 }).unwrap();
        assert_eq!((g.seats[0].score, g.seats[2].score, g.seats[1].score), (10, 10, 0));
        assert!(g.winner.is_none());
        g.apply(&db, 1, Action::Score { seat: 0, delta: 1 }).unwrap();
        assert!(g.won(0) && g.won(2) && !g.won(1));
        assert!(g.log.iter().any(|l| l.text.starts_with("Ann & Cy win the game")));
    }

    #[test]
    fn teams_burn_out_scores_the_other_team_once() {
        let db = db();
        let mut g = four_into_play(&db, Mode::Teams);
        let deck: Vec<CardId> = g.pile(Zone::Deck(0)).to_vec();
        for id in deck {
            g.place(id, Zone::Trash(0), Pos::Top);
        }
        g.apply(&db, 0, Action::Draw { n: 1 }).unwrap();
        assert_eq!((g.seats[1].score, g.seats[3].score, g.seats[2].score), (1, 1, 0));
        let mut f = four_into_play(&db, Mode::FreeForAll);
        let deck: Vec<CardId> = f.pile(Zone::Deck(0)).to_vec();
        for id in deck {
            f.place(id, Zone::Trash(0), Pos::Top);
        }
        f.apply(&db, 0, Action::Draw { n: 1 }).unwrap();
        assert_eq!(f.seats.iter().map(|s| s.score).collect::<Vec<_>>(), vec![0, 1, 1, 1]);
    }

    #[test]
    fn teammates_do_not_fight_and_share_hidden_info() {
        let db = db();
        let mut g = four_into_play(&db, Mode::Teams);
        let code = unit(&db, &g);
        let a = g.active;
        let ally = (a + 2) % 4;
        let foe = (a + 1) % 4;
        for s in [a, ally] {
            let id = g.spawn(&code, s, Zone::Base(s), false);
            g.apply(&db, a, Action::Move { card: id, to: Zone::Bf(0, s), pos: Pos::Top }).unwrap();
        }
        // Only one team present: a showdown, then the mover's team conquers.
        g.apply(&db, a, Action::StartShowdown { bf: 0 }).unwrap();
        assert!(!g.showdown.as_ref().unwrap().combat);
        g.apply(&db, a, Action::EndShowdown).unwrap();
        assert!(g.battlefields[0].controller.map(|c| g.allied(c, a)).unwrap_or(false));
        assert_eq!(g.seats[a as usize].score, 1);
        assert_eq!(g.seats[ally as usize].score, 1);
        // An enemy unit makes it a combat; allies' might adds up against it.
        let e = g.spawn(&code, foe, Zone::Bf(0, foe), false);
        g.apply(&db, a, Action::StartShowdown { bf: 0 }).unwrap();
        assert!(g.showdown.as_ref().unwrap().combat);
        g.apply(&db, a, Action::PassFocus).unwrap();
        assert_eq!(g.showdown.as_ref().unwrap().focus, (a + 1) % 4);
        g.apply(&db, a, Action::EndShowdown).unwrap();
        assert_eq!(g.cards[e as usize].zone, Zone::Trash(foe));
        let mine: Vec<CardId> = g.pile(Zone::Bf(0, a)).iter().chain(g.pile(Zone::Bf(0, ally))).copied().collect();
        assert!(mine.iter().all(|&id| g.cards[id as usize].damage < g.might(&db, id)), "teammates never damage each other");
        // Hands: visible to the teammate, hidden from opponents.
        let v = g.view_for(Some(ally), false);
        assert!(v.hand(a).iter().all(|&id| !v.cards[id as usize].code.is_empty()));
        let v = g.view_for(Some(foe), false);
        assert!(v.hand(a).iter().all(|&id| v.cards[id as usize].code.is_empty()));
        // Free-for-all keeps hands private.
        let f = four_into_play(&db, Mode::FreeForAll);
        let v = f.view_for(Some(2), false);
        assert!(v.hand(0).iter().all(|&id| v.cards[id as usize].code.is_empty()));
    }

    #[test]
    fn teams_final_point_ignores_battlefields_the_ally_holds() {
        let db = db();
        let mut g = four_into_play(&db, Mode::Teams);
        let a = g.active;
        let ally = (a + 2) % 4;
        g.apply(&db, a, Action::Score { seat: a, delta: 10 }).unwrap();
        g.battlefields[1].controller = Some(ally);
        g.battlefields[2].scored[a as usize] = g.turn;
        g.apply(&db, a, Action::Conquer { bf: 0 }).unwrap();
        assert!(g.won(a), "bf 1 is held by the ally and bf 2 was scored this turn");
        let mut g = four_into_play(&db, Mode::Teams);
        let a = g.active;
        g.apply(&db, a, Action::Score { seat: a, delta: 10 }).unwrap();
        g.battlefields[1].controller = Some((a + 1) % 4);
        g.battlefields[2].scored[a as usize] = g.turn;
        g.apply(&db, a, Action::Conquer { bf: 0 }).unwrap();
        assert!(g.winner.is_none(), "an enemy-held battlefield still blocks the final point");
    }

    #[test]
    fn team_concede_ends_game_but_free_for_all_goes_on() {
        let db = db();
        let mut g = four_into_play(&db, Mode::Teams);
        g.apply(&db, 1, Action::Concede).unwrap();
        assert!(g.seats[3].conceded);
        assert!(g.won(0) && g.won(2));
        let mut f = four_into_play(&db, Mode::FreeForAll);
        let a = f.active;
        f.apply(&db, a, Action::Concede).unwrap();
        assert!(f.winner.is_none());
        assert_eq!(f.active, (a + 1) % 4, "the conceding player's turn passes on");
        let next = f.active;
        f.apply(&db, next, Action::EndTurn).unwrap();
        f.apply(&db, f.active, Action::EndTurn).unwrap();
        f.apply(&db, f.active, Action::EndTurn).unwrap();
        assert_eq!(f.active, next, "conceded seats are skipped");
        f.apply(&db, (a + 2) % 4, Action::Concede).unwrap();
        assert!(f.winner.is_none());
        f.apply(&db, (a + 3) % 4, Action::Concede).unwrap();
        assert_eq!(f.winner, Some((a + 1) % 4));
        // Conceding during setup fills in the setup steps.
        let mut s = four_player(&db, Mode::FreeForAll);
        let Stage::ChooseOrder { chooser } = s.stage else { panic!() };
        s.apply(&db, chooser, Action::Concede).unwrap();
        assert_eq!(s.stage, Stage::ChooseBattlefield);
        let first = s.first;
        for k in 1..4u8 {
            let x = (first + k) % 4;
            if x != chooser {
                let b = s.seats[x as usize].bf_options[0].clone();
                s.apply(&db, x, Action::ChooseBattlefield { code: b }).unwrap();
            }
        }
        assert_eq!(s.stage, Stage::Mulligan);
        for x in 0..4u8 {
            if x != chooser {
                s.apply(&db, x, Action::Mulligan { cards: vec![] }).unwrap();
            }
        }
        assert_eq!(s.stage, Stage::Play);
    }

    #[test]
    fn three_player_free_for_all() {
        let db = db();
        let seats = ["SFD-181", "SFD-205", "UNL-193"]
            .iter()
            .zip(["Ann", "Bo", "Cy"])
            .map(|(l, n)| {
                let d = starter_deck(&db, l).unwrap();
                NewSeat { name: n.into(), bf_options: d.battlefields.clone(), deck: d }
            })
            .collect();
        let mut g = GameState::new(&db, seats, GameOptions { mode: Mode::Skirmish, ..Default::default() }, 3, None);
        assert!(g.multi());
        let Stage::ChooseOrder { chooser } = g.stage else { panic!("{:?}", g.stage) };
        g.apply(&db, chooser, Action::ChooseOrder { go_first: true }).unwrap();
        let first = g.first;
        for k in 1..3u8 {
            let s = (first + k) % 3;
            let b = g.seats[s as usize].bf_options[0].clone();
            g.apply(&db, s, Action::ChooseBattlefield { code: b }).unwrap();
        }
        assert_eq!(g.battlefields.len(), 2, "one battlefield per player except the first");
        for s in 0..3u8 {
            g.apply(&db, s, Action::Mulligan { cards: vec![] }).unwrap();
        }
        assert_eq!(g.hand(first).len(), 4);
        g.apply(&db, first, Action::EndTurn).unwrap();
        let second = g.active;
        assert_eq!(g.pile(Zone::Runes(second)).len(), 2);
        g.apply(&db, second, Action::EndTurn).unwrap();
        let last = g.active;
        assert_eq!(last, (first + 2) % 3);
        assert_eq!(g.pile(Zone::Runes(last)).len(), 3, "last player channels an extra rune");
        assert!(!g.allied(0, 1) && !g.allied(1, 2));
        g.apply(&db, last, Action::Concede).unwrap();
        assert!(g.winner.is_none());
        assert_eq!(g.active, first, "the conceding player's turn passes on");
        g.apply(&db, second, Action::Concede).unwrap();
        assert_eq!(g.winner, Some(first));
    }

    #[test]
    fn shown_hand_is_visible_to_every_opponent() {
        let db = db();
        for mode in [Mode::FreeForAll, Mode::Teams] {
            let mut g = four_into_play(&db, mode);
            g.apply(&db, 0, Action::RevealHand { shown: true }).unwrap();
            for v in [Some(1), Some(2), Some(3), None] {
                let view = g.view_for(v, false);
                assert!(view.hand(0).iter().all(|&id| !view.cards[id as usize].code.is_empty()), "{mode:?} viewer {v:?}");
            }
        }
    }
}
