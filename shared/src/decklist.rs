//! Canonical deck representation, deck-code/text conversion and validation.

use crate::cards::{norm_code, CardDb, CardDef};
use crate::deckcode::{self, CodeCard, DecodedDeck};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Deck {
    #[serde(default)]
    pub name: String,
    pub legend: Option<String>,
    /// Chosen champion (starts in the champion zone; counts toward the 40).
    pub champion: Option<String>,
    /// Main deck excluding the chosen champion copy. (code, count)
    #[serde(default)]
    pub main: Vec<(String, u32)>,
    #[serde(default)]
    pub runes: Vec<(String, u32)>,
    #[serde(default)]
    pub battlefields: Vec<String>,
    #[serde(default)]
    pub side: Vec<(String, u32)>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum DeckRules {
    #[default]
    Standard,
    Unrestricted,
    Sealed,
}

pub const MAIN_MIN: u32 = 40;
pub const SEALED_MAIN_MIN: u32 = 25;
pub const RUNES: u32 = 12;
pub const SIDE_MAX: u32 = 8;
pub const COPY_MAX: u32 = 3;

fn add(list: &mut Vec<(String, u32)>, code: &str, n: u32) {
    if n == 0 {
        return;
    }
    if let Some(e) = list.iter_mut().find(|e| e.0 == code) {
        e.1 += n;
    } else {
        list.push((code.to_string(), n));
    }
}

/// Rune printings are numbered the same way in every set: R01 Fury, R02 Calm,
/// R03 Mind, R04 Body, R05 Chaos, R06 Order (e.g. SFD-R02a, UNL-R05). The card
/// data lacks some sets' rune printings, so an unknown one stands in as the basic
/// rune of its domain. Runes have no rules text beyond their domain.
fn rune_printing<'a>(db: &'a CardDb, code: &str) -> Option<&'a CardDef> {
    let (_, rest) = code.split_once('-')?;
    let n: usize = rest.strip_prefix('R')?.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
    let domain = ["Fury", "Calm", "Mind", "Body", "Chaos", "Order"].get(n.checked_sub(1)?)?;
    db.basic_rune(domain)
}

fn remove(list: &mut Vec<(String, u32)>, code: &str, n: u32) -> bool {
    if let Some(i) = list.iter().position(|e| e.0 == code) {
        if list[i].1 <= n {
            list.remove(i);
        } else {
            list[i].1 -= n;
        }
        return true;
    }
    false
}

impl Deck {
    pub fn main_count(&self) -> u32 {
        self.main.iter().map(|e| e.1).sum::<u32>() + self.champion.is_some() as u32
    }
    pub fn rune_count(&self) -> u32 {
        self.runes.iter().map(|e| e.1).sum()
    }
    pub fn side_count(&self) -> u32 {
        self.side.iter().map(|e| e.1).sum()
    }

    /// Add a card to the right section based on its type.
    pub fn add_card(&mut self, db: &CardDb, code: &str, n: u32) {
        let Some(c) = db.get(code) else { return };
        let code = c.code.clone();
        if c.is_legend() {
            self.legend = Some(code);
        } else if c.is_rune() {
            add(&mut self.runes, &code, n);
        } else if c.is_battlefield() {
            if !self.battlefields.contains(&code) {
                self.battlefields.push(code);
            }
        } else if c.is_champion_unit() && self.champion.is_none() && self.legend_matches(db, c) {
            self.champion = Some(code.clone());
            if n > 1 {
                add(&mut self.main, &code, n - 1);
            }
        } else {
            add(&mut self.main, &code, n);
        }
    }

    pub fn remove_card(&mut self, code: &str) {
        let code = norm_code(code);
        if remove(&mut self.main, &code, 1) || remove(&mut self.runes, &code, 1) {
            return;
        }
        if self.champion.as_deref() == Some(code.as_str()) {
            // promote another copy from main if present
            self.champion = None;
            return;
        }
        if let Some(i) = self.battlefields.iter().position(|b| *b == code) {
            self.battlefields.remove(i);
            return;
        }
        if self.legend.as_deref() == Some(code.as_str()) {
            self.legend = None;
        }
    }

    pub fn add_side(&mut self, code: &str, n: u32) {
        add(&mut self.side, &norm_code(code), n);
    }
    pub fn remove_side(&mut self, code: &str) {
        remove(&mut self.side, &norm_code(code), 1);
    }

    /// Swap one copy main -> side and one copy side -> main (sideboarding).
    pub fn swap(&mut self, out_code: &str, in_code: &str) -> Result<(), String> {
        let (o, i) = (norm_code(out_code), norm_code(in_code));
        if !self.side.iter().any(|e| e.0 == i) {
            return Err("That card is not in your sideboard.".into());
        }
        if !remove(&mut self.main, &o, 1) {
            return Err("That card is not in your main deck.".into());
        }
        remove(&mut self.side, &i, 1);
        add(&mut self.main, &i, 1);
        add(&mut self.side, &o, 1);
        Ok(())
    }

    fn legend_matches(&self, db: &CardDb, champ: &CardDef) -> bool {
        match self.legend.as_ref().and_then(|l| db.get(l)) {
            Some(l) => l.tags.iter().any(|t| champ.tags.contains(t)),
            None => true,
        }
    }

    pub fn from_code(db: &CardDb, code: &str) -> Result<Deck, String> {
        let d = deckcode::decode(code.trim()).map_err(|e| format!("Could not decode that deck code. ({e})"))?;
        let mut deck = Deck::default();
        let mut unknown = vec![];
        for c in &d.main {
            match db.get(&c.code).or_else(|| rune_printing(db, &c.code)) {
                Some(def) if def.is_legend() => deck.legend = Some(def.code.clone()),
                Some(def) if def.is_rune() => add(&mut deck.runes, &def.code, c.count),
                Some(def) if def.is_battlefield() => {
                    for _ in 0..c.count.max(1) {
                        if !deck.battlefields.contains(&def.code) {
                            deck.battlefields.push(def.code.clone());
                        }
                    }
                }
                Some(def) => add(&mut deck.main, &def.code, c.count),
                None => unknown.push(c.code.clone()),
            }
        }
        for c in &d.side {
            match db.get(&c.code) {
                Some(def) => add(&mut deck.side, &def.code, c.count),
                None => unknown.push(c.code.clone()),
            }
        }
        if let Some(ch) = &d.champion {
            if let Some(def) = db.get(ch) {
                deck.champion = Some(def.code.clone());
                remove(&mut deck.main, &def.code, 1);
            }
        }
        if !unknown.is_empty() {
            return Err(format!("Unknown cards in deck code: {}", unknown.join(", ")));
        }
        Ok(deck)
    }

    pub fn to_code(&self) -> Result<String, String> {
        let mut main: Vec<CodeCard> = vec![];
        let mut push = |code: &str, n: u32| {
            if let Some(e) = main.iter_mut().find(|e| e.code == code) {
                e.count += n;
            } else {
                main.push(CodeCard { code: code.to_string(), count: n });
            }
        };
        if let Some(l) = &self.legend {
            push(l, 1);
        }
        if let Some(c) = &self.champion {
            push(c, 1);
        }
        for (c, n) in &self.main {
            push(c, *n);
        }
        for (c, n) in &self.runes {
            push(c, *n);
        }
        for b in &self.battlefields {
            push(b, 1);
        }
        let side = self.side.iter().map(|(c, n)| CodeCard { code: c.clone(), count: *n }).collect();
        deckcode::encode(&DecodedDeck { main, side, champion: self.champion.clone(), legends: vec![] })
    }

    /// Parse a pasted text decklist (riftatlas / Piltover / TCG export style).
    /// Section headers: Legend, Champion, MainDeck/Main Deck, Battlefields, Runes/Rune Deck, Sideboard.
    /// Lines: "3 Card Name", "3x Card Name", "Card Name x3", or card codes "3 OGN-001".
    pub fn parse_text(db: &CardDb, text: &str) -> Result<Deck, String> {
        #[derive(PartialEq, Clone, Copy)]
        enum Sec {
            Auto,
            Legend,
            Champion,
            Main,
            Bf,
            Runes,
            Side,
        }
        let mut sec = Sec::Auto;
        let mut deck = Deck::default();
        let mut errors = vec![];
        let mut found_any = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let head = lower.trim_end_matches(':').trim();
            let head = head.split(" (").next().unwrap_or(head).trim();
            let new_sec = match head {
                "legend" | "legends" => Some(Sec::Legend),
                "champion" | "chosen champion" => Some(Sec::Champion),
                "maindeck" | "main deck" | "main" | "deck" => Some(Sec::Main),
                "battlefield" | "battlefields" => Some(Sec::Bf),
                "rune" | "runes" | "rune deck" | "runedeck" => Some(Sec::Runes),
                "sideboard" | "side" | "side deck" => Some(Sec::Side),
                _ => None,
            };
            if let Some(s) = new_sec {
                sec = s;
                continue;
            }
            // Inline header like "Legend: 1 Jinx, Loose Cannon"
            let (line, sec_here) = match line.split_once(':') {
                Some((h, rest)) if matches!(h.trim().to_ascii_lowercase().as_str(), "legend" | "champion" | "battlefield" | "battlefields" | "runes" | "maindeck" | "sideboard") => {
                    let s = match h.trim().to_ascii_lowercase().as_str() {
                        "legend" => Sec::Legend,
                        "champion" => Sec::Champion,
                        "battlefield" | "battlefields" => Sec::Bf,
                        "runes" => Sec::Runes,
                        "sideboard" => Sec::Side,
                        _ => Sec::Main,
                    };
                    (rest.trim(), s)
                }
                _ => (line, sec),
            };
            let (count, name) = parse_count(line);
            let def = db.get(name).or_else(|| db.by_name(name));
            let Some(def) = def else {
                errors.push(format!("Unknown card: {name}"));
                continue;
            };
            found_any = true;
            let code = def.code.clone();
            match sec_here {
                Sec::Legend => deck.legend = Some(code),
                Sec::Champion => {
                    deck.champion = Some(code.clone());
                    if count > 1 {
                        add(&mut deck.main, &code, count - 1);
                    }
                }
                Sec::Side => add(&mut deck.side, &code, count),
                Sec::Bf => {
                    if !deck.battlefields.contains(&code) {
                        deck.battlefields.push(code)
                    }
                }
                Sec::Runes => add(&mut deck.runes, &code, count),
                Sec::Main | Sec::Auto => {
                    if def.is_legend() {
                        deck.legend = Some(code)
                    } else if def.is_rune() {
                        add(&mut deck.runes, &code, count)
                    } else if def.is_battlefield() {
                        if !deck.battlefields.contains(&code) {
                            deck.battlefields.push(code)
                        }
                    } else {
                        add(&mut deck.main, &code, count)
                    }
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        if !found_any {
            return Err("No cards were found in that deck list.".into());
        }
        // If no explicit champion, pick the first champion unit matching the legend.
        if deck.champion.is_none() {
            if let Some(legend) = deck.legend.as_ref().and_then(|l| db.get(l)) {
                let pick = deck.main.iter().find(|(c, _)| db.get(c).map(|d| d.is_champion_unit() && legend.tags.iter().any(|t| d.tags.contains(t))).unwrap_or(false)).map(|e| e.0.clone());
                if let Some(p) = pick {
                    remove(&mut deck.main, &p, 1);
                    deck.champion = Some(p);
                }
            }
        }
        Ok(deck)
    }

    /// Accept either a deck code or a text list.
    pub fn import(db: &CardDb, input: &str) -> Result<Deck, String> {
        let t = input.trim();
        if t.is_empty() {
            return Err("Paste a Riftbound deck code first.".into());
        }
        if !t.contains(char::is_whitespace) && t.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Deck::from_code(db, t);
        }
        Deck::parse_text(db, t)
    }

    pub fn to_text(&self, db: &CardDb) -> String {
        let name = |c: &str| db.get(c).map(|d| d.name.clone()).unwrap_or_else(|| c.to_string());
        let mut s = String::new();
        if let Some(l) = &self.legend {
            s += &format!("Legend:\n1 {}\n", name(l));
        }
        if let Some(c) = &self.champion {
            s += &format!("Champion:\n1 {}\n", name(c));
        }
        s += "MainDeck:\n";
        for (c, n) in &self.main {
            s += &format!("{} {}\n", n, name(c));
        }
        s += "Battlefields:\n";
        for b in &self.battlefields {
            s += &format!("1 {}\n", name(b));
        }
        s += "Runes:\n";
        for (c, n) in &self.runes {
            s += &format!("{} {}\n", n, name(c));
        }
        if !self.side.is_empty() {
            s += "Sideboard:\n";
            for (c, n) in &self.side {
                s += &format!("{} {}\n", n, name(c));
            }
        }
        s
    }

    /// Validate against a rule set. Returns a list of human-readable problems (empty = legal).
    pub fn validate(&self, db: &CardDb, rules: DeckRules) -> Vec<String> {
        let mut e = vec![];
        let legend = self.legend.as_ref().and_then(|l| db.get(l));
        if legend.is_none() {
            e.push("The deck needs a Legend.".into());
        }
        let main_n = self.main_count();
        if self.main.is_empty() && self.champion.is_none() {
            e.push("No main deck cards were found.".into());
        }
        if self.rune_count() == 0 {
            e.push("The rune deck is empty.".into());
        }
        if rules == DeckRules::Unrestricted {
            return e;
        }
        let allowed: Vec<String> = legend.map(|l| l.domains.clone()).unwrap_or_default();
        let in_identity = |d: &CardDef| d.is_colorless() || d.domains.iter().all(|x| x == "Colorless" || allowed.contains(x));
        let min = if rules == DeckRules::Sealed { SEALED_MAIN_MIN } else { MAIN_MIN };
        if main_n < min {
            e.push(format!("The main deck has {main_n} cards (at least {min} required, including the champion)."));
        }
        if self.rune_count() != RUNES {
            e.push(format!("The rune deck has {} runes (exactly {RUNES} required).", self.rune_count()));
        }
        if rules == DeckRules::Standard {
            if self.battlefields.len() != 3 {
                e.push(format!("Choose exactly 3 battlefields ({} selected).", self.battlefields.len()));
            }
            match self.champion.as_ref().and_then(|c| db.get(c)) {
                None => e.push("Choose a Chosen Champion.".into()),
                Some(ch) => {
                    if !ch.is_champion_unit() {
                        e.push(format!("{} is not a champion unit.", ch.name));
                    } else if let Some(l) = legend {
                        if !l.tags.iter().any(|t| ch.tags.contains(t)) {
                            e.push(format!("{} does not match the legend {}.", ch.name, l.name));
                        }
                    }
                }
            }
            if self.side_count() > SIDE_MAX {
                e.push(format!("The sideboard has {} cards (max {SIDE_MAX}).", self.side_count()));
            }
            let mut by_name: BTreeMap<String, u32> = BTreeMap::new();
            let mut sig = 0;
            for (c, n) in self.main.iter().chain(self.side.iter()).chain(self.champion.iter().map(|c| (c.clone(), 1)).collect::<Vec<_>>().iter()) {
                if let Some(d) = db.get(c) {
                    *by_name.entry(d.name.clone()).or_default() += n;
                    if d.is_signature() {
                        sig += n;
                        if let Some(l) = legend {
                            if !l.tags.iter().any(|t| d.tags.contains(t)) {
                                e.push(format!("Signature card {} does not match your legend.", d.name));
                            }
                        }
                    }
                }
            }
            for (n, c) in by_name {
                if c > COPY_MAX {
                    e.push(format!("{n}: {c} copies (max {COPY_MAX})."));
                }
            }
            if sig > 3 {
                e.push(format!("{sig} signature cards (max 3)."));
            }
        } else if self.battlefields.is_empty() {
            e.push("No battlefields were found in the deck list.".into());
        }
        for (c, _) in self.main.iter().chain(self.side.iter()) {
            match db.get(c) {
                None => e.push(format!("Unknown card {c}.")),
                Some(d) => {
                    if !d.is_main_deck_card() {
                        e.push(format!("{} cannot be in the main deck.", d.name));
                    } else if legend.is_some() && !in_identity(d) {
                        e.push(format!("{} is outside your legend's domains.", d.name));
                    }
                }
            }
        }
        for (c, _) in &self.runes {
            if let Some(d) = db.get(c) {
                if !d.is_rune() {
                    e.push(format!("{} is not a rune.", d.name));
                } else if legend.is_some() && !in_identity(d) {
                    e.push(format!("{} does not match your legend's domains.", d.name));
                }
            }
        }
        e
    }
}

fn parse_count(line: &str) -> (u32, &str) {
    let l = line.trim();
    // "3 Name" / "3x Name"
    let digits: String = l.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let rest = l[digits.len()..].trim_start();
        let rest = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')).map(|r| r.trim_start()).unwrap_or(rest);
        if !rest.is_empty() {
            return (digits.parse().unwrap_or(1), rest);
        }
    }
    // "Name x3"
    if let Some(p) = l.rfind([' ', '\t']) {
        let tail = l[p + 1..].trim_start_matches(['x', 'X']);
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) && l[p + 1..].len() > tail.len() {
            return (tail.parse().unwrap_or(1), l[..p].trim());
        }
    }
    (1, l)
}

/// A playable starter deck built from the card pool for a legend; used for
/// goldfish opponents, tests and "quick deck" in the editor.
/// Riot's preconstructed deck for a legend (Champion Deck or Showdown Deck).
/// Legends without one have no starter deck.
pub fn starter_deck(db: &CardDb, legend_code: &str) -> Option<Deck> {
    crate::precons::deck_for(db, legend_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::tests::db;

    #[test]
    fn imports_rune_printings_missing_from_the_card_data() {
        let db = db();
        // A Riven deck from an online deck builder, with Unleashed alternate-art runes.
        let code = "CQAAAAAAAAAACAQEAEAQEAIFAAAAKAYAAAAC2AB2ACUQCAIDAAAIEAIBAQAAANICAYAAA2IASYAQCBQBABZAGAIAAAAEGAIDAAADABAGAAABYABAAARAA2YFAEBQAAGPAEAQIAAA24AQCBABACJQCBAGAAACSADFACRQCAFNAEAQMAIAFMAAIAIEAAAIAAIBAQAQBFQBAECQAABIAEDAAADFAIAQGAAAFUAQMAAAFEAQMAIAFM";
        assert!(db.get("UNL-R02a").is_none() && db.get("UNL-R05a").is_none());
        let d = Deck::from_code(&db, code).unwrap();
        let rune_doms: Vec<(String, u32)> = d.runes.iter().map(|(c, n)| (db.get(c).unwrap().domains[0].clone(), *n)).collect();
        assert_eq!(rune_doms, vec![("Calm".to_string(), 6), ("Chaos".to_string(), 6)]);
        assert_eq!(db.get(d.legend.as_ref().unwrap()).unwrap().name, "Riven, The Exile");
        assert_eq!(d.main_count(), 40);
        // The list itself carries a 10-card sideboard; that is the only problem.
        assert_eq!(d.validate(&db, DeckRules::Standard), vec!["The sideboard has 10 cards (max 8).".to_string()]);
        // Same numbering as the rune printings the data does have.
        for (code, dom) in [("VEN-R01", "Fury"), ("VEN-R02", "Calm"), ("RAD-R03", "Mind"), ("RAD-R04", "Body"), ("VEN-R05", "Chaos"), ("RAD-R06", "Order")] {
            assert_eq!(db.get(code).unwrap().domains, vec![dom.to_string()]);
        }
    }

    #[test]
    fn starters_are_riots_decks() {
        let db = db();
        for p in crate::precons::all() {
            let d = starter_deck(&db, &p.legend).unwrap();
            assert_eq!(d.name, p.name);
            assert!(d.validate(&db, DeckRules::Standard).is_empty());
        }
        assert!(starter_deck(&db, "OGN-251").is_none(), "no Riot deck for Jinx, Loose Cannon in the data");
    }

    #[test]
    fn code_roundtrip_preserves_deck() {
        let db = db();
        let d = starter_deck(&db, "SFD-181").unwrap();
        let code = d.to_code().unwrap();
        let back = Deck::from_code(&db, &code).unwrap();
        assert_eq!(back.legend, d.legend);
        assert_eq!(back.champion, d.champion);
        assert_eq!(back.main_count(), d.main_count());
        assert_eq!(back.rune_count(), 12);
        assert_eq!(back.battlefields.len(), 3);
    }

    #[test]
    fn text_roundtrip_and_formats() {
        let db = db();
        let d = starter_deck(&db, "UNL-193").unwrap();
        let t = d.to_text(&db);
        let back = Deck::parse_text(&db, &t).unwrap();
        assert_eq!(back.main_count(), d.main_count());
        assert_eq!(back.champion, d.champion);
        let alt = Deck::parse_text(&db, "Legend: Jinx - Loose Cannon\n3x Blazing Scorcher\nBlazing Scorcher x2\n6 Fury Rune").unwrap();
        assert_eq!(alt.legend.as_deref(), Some("OGN-251"));
        assert_eq!(alt.main, vec![("OGN-001".to_string(), 5)]);
        assert_eq!(alt.rune_count(), 6);
        assert!(Deck::parse_text(&db, "3 Not A Real Card").is_err());
    }

    #[test]
    fn validation_catches_problems() {
        let db = db();
        let mut d = starter_deck(&db, "SFD-181").unwrap();
        d.main[0].1 = 5;
        d.runes[0].1 = 2;
        d.battlefields.pop();
        let errs = d.validate(&db, DeckRules::Standard);
        assert!(errs.iter().any(|e| e.contains("copies")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("rune deck")));
        assert!(errs.iter().any(|e| e.contains("battlefields")));
        assert!(d.validate(&db, DeckRules::Unrestricted).is_empty());
    }

    #[test]
    fn sideboard_swap() {
        let db = db();
        let mut d = starter_deck(&db, "SFD-181").unwrap();
        let out = d.main[0].0.clone();
        d.add_side("OGN-002", 1);
        d.swap(&out, "OGN-002").unwrap();
        assert!(d.main.iter().any(|e| e.0 == "OGN-002"));
        assert!(d.side.iter().any(|e| e.0 == out));
        assert!(d.swap(&out, "OGN-999").is_err());
    }
}
