//! Riot's preconstructed decks (Champion Decks and Showdown Decks), as
//! published on playriftbound.com. Built from the published lists by
//! `data/build_precons.py`; only decks whose every card could be matched
//! exactly are included.

use crate::cards::CardDb;
use crate::decklist::Deck;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize, Clone, Debug)]
pub struct Precon {
    /// e.g. "Rumble Champion Deck".
    pub name: String,
    /// e.g. "Spiritforged Champion Deck".
    pub product: String,
    /// Where Riot published the list.
    pub source: String,
    pub legend: String,
    /// Riot lists the champion units as options; the first one listed starts
    /// as the Chosen Champion and the rest are in the main deck.
    pub champion_options: Vec<(String, u32)>,
    pub main: Vec<(String, u32)>,
    pub battlefields: Vec<String>,
    pub runes: Vec<(String, u32)>,
}

pub fn all() -> &'static [Precon] {
    static P: OnceLock<Vec<Precon>> = OnceLock::new();
    P.get_or_init(|| serde_json::from_str(include_str!("../../data/precons.json")).expect("data/precons.json"))
}

impl Precon {
    pub fn deck(&self) -> Deck {
        let mut main: Vec<(String, u32)> = vec![];
        let mut champion = None;
        for (i, (c, n)) in self.champion_options.iter().enumerate() {
            let n = if i == 0 {
                champion = Some(c.clone());
                n - 1
            } else {
                *n
            };
            if n > 0 {
                main.push((c.clone(), n));
            }
        }
        main.extend(self.main.iter().cloned());
        Deck { name: self.name.clone(), legend: Some(self.legend.clone()), champion, main, runes: self.runes.clone(), battlefields: self.battlefields.clone(), side: vec![] }
    }
}

/// The preconstructed deck for a legend, if Riot made one.
pub fn for_legend(legend: &str) -> Option<&'static Precon> {
    all().iter().find(|p| p.legend == legend)
}

/// Deck of Riot's preconstructed deck for `legend`.
pub fn deck_for(_db: &CardDb, legend: &str) -> Option<Deck> {
    for_legend(legend).map(|p| p.deck())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::tests::db;
    use crate::decklist::DeckRules;

    #[test]
    fn every_precon_is_a_legal_56_card_deck() {
        let db = db();
        assert_eq!(all().len(), 5, "Rumble, Fiora, Vex, Vi, Shen");
        for p in all() {
            let d = p.deck();
            assert_eq!(d.main_count(), 40, "{}", p.name);
            assert_eq!(d.rune_count(), 12, "{}", p.name);
            assert_eq!(d.battlefields.len(), 3, "{}", p.name);
            let v = d.validate(&db, DeckRules::Standard);
            assert!(v.is_empty(), "{}: {v:?}", p.name);
            assert!(p.source.starts_with("https://playriftbound.com/"));
        }
    }
}
