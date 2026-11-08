//! Sealed pack generation and sealed pool deck checks.

use crate::cards::{CardDb, CardDef};
use crate::decklist::Deck;
use crate::rng::{seed_from_str, Rng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Sets that have their own booster products.
pub const BOOSTER_SETS: &[(&str, &str)] = &[("OGN", "Origins"), ("SFD", "Spiritforged"), ("UNL", "Unleashed"), ("VEN", "Vendetta"), ("RAD", "Radiance")];
pub const DEFAULT_PACKS: usize = 6;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SealedPool {
    pub seed: String,
    pub sets: Vec<String>,
    /// Pack contents (card codes), in opening order.
    pub packs: Vec<Vec<String>>,
}

impl SealedPool {
    pub fn cards(&self) -> HashMap<String, u32> {
        let mut m = HashMap::new();
        for p in &self.packs {
            for c in p {
                *m.entry(c.clone()).or_insert(0) += 1;
            }
        }
        m
    }
}

fn pool_of<'a>(db: &'a CardDb, set: &str, f: impl Fn(&CardDef) -> bool) -> Vec<&'a CardDef> {
    db.cards.iter().filter(|c| c.set == set && !c.alt && !c.is_token() && !c.is_rune() && f(c)).collect()
}

fn pick<'a>(r: &mut Rng, v: &[&'a CardDef]) -> Option<&'a CardDef> {
    if v.is_empty() {
        None
    } else {
        Some(v[r.below(v.len() as u64) as usize])
    }
}

/// Open one 14-card booster: 7 commons, 3 uncommons, 2 rare-or-better
/// (about 1 in 4 is epic), 1 foil of any rarity, 1 rune.
pub fn open_pack(db: &CardDb, set: &str, r: &mut Rng) -> Vec<String> {
    let commons = pool_of(db, set, |c| c.rarity == "Common");
    let uncommons = pool_of(db, set, |c| c.rarity == "Uncommon");
    let rares = pool_of(db, set, |c| c.rarity == "Rare");
    let epics = pool_of(db, set, |c| c.rarity == "Epic");
    let all = pool_of(db, set, |_| true);
    let runes: Vec<&CardDef> = db.cards.iter().filter(|c| c.is_rune() && !c.alt && c.set == set).collect();
    let runes = if runes.is_empty() { db.cards.iter().filter(|c| c.is_rune() && !c.alt).collect() } else { runes };
    let mut out = vec![];
    let take_distinct = |out: &mut Vec<String>, src: &[&CardDef], n: usize, r: &mut Rng| {
        let mut tries = 0;
        let mut got = 0;
        while got < n && tries < 200 {
            tries += 1;
            if let Some(c) = pick(r, src) {
                if !out.contains(&c.code) || src.len() < n {
                    out.push(c.code.clone());
                    got += 1;
                }
            } else {
                break;
            }
        }
    };
    take_distinct(&mut out, &commons, 7, r);
    take_distinct(&mut out, &uncommons, 3, r);
    for _ in 0..2 {
        let src = if r.below(4) == 0 && !epics.is_empty() { &epics } else { &rares };
        take_distinct(&mut out, src, 1, r);
    }
    take_distinct(&mut out, &all, 1, r);
    if let Some(c) = pick(r, &runes) {
        out.push(c.code.clone());
    }
    out
}

pub fn open_pool(db: &CardDb, sets: &[String], seed: &str) -> SealedPool {
    let mut r = Rng::new(seed_from_str(seed));
    let packs = sets.iter().map(|s| open_pack(db, s, &mut r)).collect();
    SealedPool { seed: seed.to_string(), sets: sets.to_vec(), packs }
}

/// Default sealed product: six packs of one set.
pub fn default_sets(set: &str) -> Vec<String> {
    vec![set.to_string(); DEFAULT_PACKS]
}

/// Check that a sealed deck only uses cards from its pool (basic runes are free).
pub fn check_pool(db: &CardDb, deck: &Deck, pool: &SealedPool) -> Vec<String> {
    let mut have = pool.cards();
    let mut errs = vec![];
    let mut need: Vec<(String, u32)> = deck.main.clone();
    need.extend(deck.side.iter().cloned());
    if let Some(c) = &deck.champion {
        need.push((c.clone(), 1));
    }
    // The legend may come from outside the pool (pools often lack one), and
    // battlefields are only checked when the pool opened any.
    let pool_has_bf = have.keys().any(|c| db.get(c).map(|d| d.is_battlefield()).unwrap_or(false));
    if pool_has_bf {
        for b in &deck.battlefields {
            need.push((b.clone(), 1));
        }
    }
    for (c, n) in need {
        let h = have.entry(c.clone()).or_insert(0);
        if *h < n {
            let name = db.get(&c).map(|d| d.name.clone()).unwrap_or(c.clone());
            errs.push(format!("{name} is not in this sealed pool ({} of {n}).", *h));
        } else {
            *h -= n;
        }
    }
    for (c, _) in &deck.runes {
        if !db.get(c).map(|d| d.is_rune()).unwrap_or(false) {
            errs.push(format!("{c} is not a rune."));
        }
    }
    errs
}

/// Build a quick legal sealed deck from a pool (used by auto-build and tests).
pub fn auto_build(db: &CardDb, pool: &SealedPool) -> Option<Deck> {
    let have = pool.cards();
    let mut legends: Vec<&CardDef> = have.keys().filter_map(|c| db.get(c)).filter(|d| d.is_legend()).collect();
    legends.sort_by_key(|d| d.code.clone());
    // If the pool has no legend, sealed play lets you use any legend of the pool's best domains.
    let legend = match legends.first() {
        Some(l) => (*l).clone(),
        None => {
            let mut dom: HashMap<String, u32> = HashMap::new();
            for (c, n) in &have {
                if let Some(d) = db.get(c) {
                    for x in &d.domains {
                        if x != "Colorless" {
                            *dom.entry(x.clone()).or_default() += n;
                        }
                    }
                }
            }
            let mut doms: Vec<(String, u32)> = dom.into_iter().collect();
            doms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let top: Vec<String> = doms.iter().take(2).map(|d| d.0.clone()).collect();
            db.cards.iter().filter(|c| c.is_legend() && !c.alt && c.domains.iter().all(|d| top.contains(d))).min_by_key(|c| c.code.clone())?.clone()
        }
    };
    let mut deck = Deck { name: format!("Sealed {}", pool.seed), legend: Some(legend.code.clone()), ..Default::default() };
    let mut cards: Vec<(&CardDef, u32)> = have
        .iter()
        .filter_map(|(c, n)| db.get(c).map(|d| (d, *n)))
        .filter(|(d, _)| d.is_main_deck_card() && (d.is_colorless() || d.domains.iter().all(|x| legend.domains.contains(x))))
        .collect();
    cards.sort_by_key(|(d, _)| (d.energy.unwrap_or(9), d.code.clone()));
    for (d, n) in cards {
        deck.main.push((d.code.clone(), n));
    }
    // Fill to 25 with off-domain cards only if needed (deck may be unplayable otherwise).
    deck.battlefields = have.keys().filter(|c| db.get(c).map(|d| d.is_battlefield()).unwrap_or(false)).cloned().collect();
    deck.battlefields.sort();
    if deck.battlefields.is_empty() {
        deck.battlefields = db.cards.iter().filter(|c| c.is_battlefield() && !c.alt).take(1).map(|c| c.code.clone()).collect();
    }
    let n = legend.domains.len().max(1) as u32;
    for (i, d) in legend.domains.iter().enumerate() {
        let r = db.basic_rune(d)?;
        let cnt = 12 / n + if (i as u32) < 12 % n { 1 } else { 0 };
        deck.runes.push((r.code.clone(), cnt));
    }
    Some(deck)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::tests::db;

    #[test]
    fn packs_are_deterministic_and_shaped() {
        let db = db();
        let a = open_pool(&db, &default_sets("OGN"), "seed-1");
        let b = open_pool(&db, &default_sets("OGN"), "seed-1");
        let c = open_pool(&db, &default_sets("OGN"), "seed-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.packs.len(), 6);
        for p in &a.packs {
            assert_eq!(p.len(), 14, "{p:?}");
            let commons = p.iter().take(7).filter(|c| db.get(c).unwrap().rarity == "Common").count();
            assert_eq!(commons, 7);
            assert!(db.get(p.last().unwrap()).unwrap().is_rune());
        }
        for s in BOOSTER_SETS {
            let p = open_pool(&db, &[s.0.to_string()], "x");
            assert_eq!(p.packs[0].len(), 14, "{}", s.0);
        }
    }

    #[test]
    fn pool_check() {
        let db = db();
        let pool = open_pool(&db, &default_sets("OGN"), "abc");
        let deck = auto_build(&db, &pool).unwrap();
        let errs = check_pool(&db, &deck, &pool);
        assert!(errs.is_empty(), "{errs:?}");
        assert!(deck.validate(&db, crate::decklist::DeckRules::Sealed).iter().all(|e| e.contains("main deck has")), "{:?}", deck.validate(&db, crate::decklist::DeckRules::Sealed));
        let mut bad = deck.clone();
        bad.main.push(("OGN-999".into(), 3));
        assert!(!check_pool(&db, &bad, &pool).is_empty());
    }
}
