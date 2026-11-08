//! Card database loaded from data/cards.json (built by data/fetch_cards.py).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CardDef {
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub full: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "super", default)]
    pub sup: String,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub domains: Vec<String>,
    pub energy: Option<i32>,
    pub might: Option<i32>,
    pub power: Option<i32>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub set: String,
    #[serde(default)]
    pub num: u32,
    #[serde(default)]
    pub img: String,
    #[serde(default)]
    pub land: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub sig: bool,
}

impl CardDef {
    pub fn is_legend(&self) -> bool {
        self.kind == "Legend"
    }
    pub fn is_rune(&self) -> bool {
        self.kind == "Rune"
    }
    pub fn is_battlefield(&self) -> bool {
        self.kind == "Battlefield"
    }
    pub fn is_unit(&self) -> bool {
        self.kind == "Unit"
    }
    pub fn is_spell(&self) -> bool {
        self.kind == "Spell"
    }
    pub fn is_gear(&self) -> bool {
        self.kind == "Gear"
    }
    pub fn is_token(&self) -> bool {
        self.sup == "Token"
    }
    pub fn is_champion_unit(&self) -> bool {
        self.kind == "Unit" && self.sup == "Champion"
    }
    pub fn is_signature(&self) -> bool {
        self.sup == "Signature"
    }
    /// Main-deck card: not legend/rune/battlefield/token.
    pub fn is_main_deck_card(&self) -> bool {
        !(self.is_legend() || self.is_rune() || self.is_battlefield() || self.is_token())
    }
    /// The name shared by every printing (used for copy limits).
    pub fn base_name(&self) -> &str {
        &self.name
    }
    pub fn is_colorless(&self) -> bool {
        self.domains.is_empty() || self.domains.iter().all(|d| d == "Colorless")
    }
}

#[derive(Clone, Debug, Default)]
pub struct CardDb {
    pub cards: Vec<CardDef>,
    by_code: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
}

/// Normalise a name for lookup: lower-case, " - " == ", ", strip punctuation noise.
pub fn norm_name(s: &str) -> String {
    let s = s.trim().replace(" - ", ", ").replace('’', "'");
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim_end().to_string()
}

/// Canonical code: upper-case set, '*' signed suffix -> 's'.
pub fn norm_code(code: &str) -> String {
    let code = code.trim();
    let mut parts = code.splitn(2, '-');
    let set = parts.next().unwrap_or("").to_ascii_uppercase();
    let rest = parts.next().unwrap_or("");
    let mut rest = rest.to_string();
    if rest.ends_with('*') {
        rest.pop();
        rest.push('s');
    }
    // prefixes R / SP / T are upper case, variant suffix lower case
    let (prefix, tail) = if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case("sp") {
        ("SP", &rest[2..])
    } else if !rest.is_empty() && (rest[..1].eq_ignore_ascii_case("r") || rest[..1].eq_ignore_ascii_case("t")) {
        (if rest[..1].eq_ignore_ascii_case("r") { "R" } else { "T" }, &rest[1..])
    } else {
        ("", &rest[..])
    };
    format!("{}-{}{}", set, prefix, tail.to_ascii_lowercase())
}

impl CardDb {
    pub fn from_json(json: &str) -> Result<CardDb, String> {
        let cards: Vec<CardDef> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        Ok(Self::from_cards(cards))
    }

    pub fn from_cards(cards: Vec<CardDef>) -> CardDb {
        let mut by_code = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in cards.iter().enumerate() {
            by_code.insert(norm_code(&c.code), i);
            by_name.entry(norm_name(&c.name)).or_default().push(i);
            let full = norm_name(&c.full);
            if !c.full.is_empty() && full != norm_name(&c.name) {
                by_name.entry(full).or_default().push(i);
            }
        }
        // Prefer base printings (no variant) first for name lookups.
        for v in by_name.values_mut() {
            v.sort_by_key(|&i| (cards[i].alt, cards[i].set == "OPP" || cards[i].set == "PR", cards[i].code.clone()));
        }
        CardDb { cards, by_code, by_name }
    }

    pub fn get(&self, code: &str) -> Option<&CardDef> {
        self.by_code.get(&norm_code(code)).map(|&i| &self.cards[i])
    }

    pub fn by_name(&self, name: &str) -> Option<&CardDef> {
        let n = norm_name(name);
        if let Some(v) = self.by_name.get(&n) {
            return v.first().map(|&i| &self.cards[i]);
        }
        // Strip a trailing parenthetical, e.g. "Jinx, Loose Cannon (Alternate Art)"
        if let Some(p) = n.rfind(" (") {
            return self.by_name(&n[..p]);
        }
        None
    }

    pub fn tokens(&self) -> impl Iterator<Item = &CardDef> {
        self.cards.iter().filter(|c| c.is_token())
    }

    /// The basic rune printing for a domain (e.g. "Fury" -> OGN-007).
    pub fn basic_rune(&self, domain: &str) -> Option<&CardDef> {
        self.cards
            .iter()
            .filter(|c| c.is_rune() && !c.alt && c.domains.iter().any(|d| d == domain))
            .min_by_key(|c| (c.set != "OGN", c.code.clone()))
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn db() -> CardDb {
        CardDb::from_json(include_str!("../../data/cards.json")).unwrap()
    }

    #[test]
    fn loads_and_looks_up() {
        let db = db();
        assert!(db.len() > 1000);
        assert_eq!(db.get("ogn-001").unwrap().name, "Blazing Scorcher");
        assert_eq!(db.get("OGN-251").unwrap().name, "Jinx, Loose Cannon");
        assert_eq!(db.by_name("Jinx - Loose Cannon").unwrap().code, "OGN-251");
        assert_eq!(db.by_name("jinx, loose cannon").unwrap().code, "OGN-251");
        assert!(db.basic_rune("Fury").unwrap().is_rune());
    }

    /// Any HTML escape: "&name;" or "&#123;" / "&#x1F;".
    fn has_html_escape(s: &str) -> bool {
        s.match_indices('&').any(|(i, _)| {
            let rest = &s[i + 1..];
            let body: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '#').collect();
            !body.is_empty() && rest[body.len()..].starts_with(';')
        })
    }

    #[test]
    fn card_text_has_no_html_escapes() {
        let db = db();
        assert!(has_html_escape("a &gt; b") && has_html_escape("&#39;") && has_html_escape("&#x2014;") && !has_html_escape("Salt & Pepper; ok"));
        let bad: Vec<&str> = db.cards.iter().filter(|c| has_html_escape(&c.text) || has_html_escape(&c.name) || c.tags.iter().any(|t| has_html_escape(t))).map(|c| c.code.as_str()).collect();
        assert!(bad.is_empty(), "HTML escapes in {bad:?}");
        assert!(db.get("UNL-047").unwrap().text.contains("[Level 3][>]"));
    }

    #[test]
    fn normalises_codes() {
        assert_eq!(norm_code("ogn-229*"), "OGN-229s");
        assert_eq!(norm_code("ven-r01"), "VEN-R01");
        assert_eq!(norm_code("VEN-sp1A"), "VEN-SP1a");
    }
}
