//! Riftbound deck code encode/decode, compatible with the Piltover Archive
//! format (`@piltoverarchive/riftbound-deck-codes`, Apache-2.0), versions 1-6.
//! Base32 of a varint byte stream; see that library's README for the layout.

use std::cmp::Ordering;

const FORMAT: u8 = 1;
const MAX_VERSION: u8 = 6;
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const SETS: &[(&str, u8)] = &[("OGN", 0), ("OGS", 1), ("ARC", 2), ("SFD", 3), ("UNL", 4), ("VEN", 5), ("RAD", 6)];
const VARIANTS: &[(&str, u8)] = &[("", 0), ("a", 1), ("s", 2), ("b", 3)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeCard {
    pub code: String,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecodedDeck {
    pub main: Vec<CodeCard>,
    pub side: Vec<CodeCard>,
    pub champion: Option<String>,
    pub legends: Vec<String>,
}

fn b32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buf >> bits) & 31) as usize] as char);
        }
        buf &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn b32_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for ch in s.trim().chars() {
        let up = ch.to_ascii_uppercase() as u8;
        let v = B32.iter().position(|&c| c == up).ok_or_else(|| format!("Invalid character in deck code: '{ch}'"))? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
        buf &= (1 << bits) - 1;
    }
    Ok(out)
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    if v == 0 {
        out.push(0);
        return;
    }
    while v != 0 {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.i).ok_or("Deck code ended unexpectedly")?;
        self.i += 1;
        Ok(v)
    }
    fn varint(&mut self) -> Result<u32, String> {
        let (mut r, mut shift) = (0u32, 0u32);
        loop {
            let b = self.byte()?;
            if shift < 32 {
                r |= ((b & 0x7f) as u32) << shift;
            }
            if b & 0x80 == 0 {
                return Ok(r);
            }
            shift += 7;
            if shift > 63 {
                return Err("Invalid varint".into());
            }
        }
    }
}

struct Parsed {
    set: String,
    number: String,
    variant: String,
}

fn parse_code(code: &str) -> Result<Parsed, String> {
    let mut it = code.split('-');
    let (set, rest) = match (it.next(), it.next(), it.next()) {
        (Some(s), Some(r), None) if !s.is_empty() && !r.is_empty() => (s, r),
        _ => return Err(format!("Invalid card code format: {code}")),
    };
    let (prefix, body) = if let Some(b) = rest.strip_prefix("SP") {
        ("SP", b)
    } else if let Some(b) = rest.strip_prefix('R') {
        ("R", b)
    } else {
        ("", rest)
    };
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    let var = &body[digits.len()..];
    if digits.is_empty() || var.len() > 1 || (var.len() == 1 && !(var.chars().all(|c| c.is_ascii_lowercase() || c == '*'))) {
        return Err(format!("Invalid card code format: {code}"));
    }
    Ok(Parsed { set: set.to_string(), number: format!("{prefix}{digits}"), variant: if var == "*" { "s".into() } else { var.into() } })
}

fn set_id(s: &str) -> Result<u8, String> {
    SETS.iter().find(|(n, _)| *n == s).map(|x| x.1).ok_or_else(|| format!("Unknown set: {s}"))
}
fn set_name(id: u8) -> Result<&'static str, String> {
    SETS.iter().find(|x| x.1 == id).map(|x| x.0).ok_or_else(|| format!("Unknown set code: {id}"))
}
fn var_id(v: &str) -> Result<u8, String> {
    VARIANTS.iter().find(|(n, _)| *n == v).map(|x| x.1).ok_or_else(|| format!("Unknown variant: '{v}'"))
}
fn var_name(id: u8) -> Result<&'static str, String> {
    VARIANTS.iter().find(|x| x.1 == id).map(|x| x.0).ok_or_else(|| format!("Unknown variant code: {id}"))
}

/// Natural compare (digit runs numerically), matching localeCompare({numeric:true}) for these codes.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let (va, vb) = (na.parse::<u64>().unwrap_or(0), nb.parse::<u64>().unwrap_or(0));
                if va != vb {
                    return va.cmp(&vb);
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

struct Group {
    set: u8,
    var: u8,
    nums: Vec<String>,
}

fn group(cards: &[&CodeCard]) -> Result<Vec<Group>, String> {
    let mut groups: Vec<Group> = Vec::new();
    for c in cards {
        let p = parse_code(&c.code)?;
        let (s, v) = (set_id(&p.set)?, var_id(&p.variant)?);
        match groups.iter_mut().find(|g| g.set == s && g.var == v) {
            Some(g) => g.nums.push(p.number),
            None => groups.push(Group { set: s, var: v, nums: vec![p.number] }),
        }
    }
    groups.sort_by_key(|g| (g.set, g.var));
    for g in &mut groups {
        g.nums.sort_by(|a, b| natural_cmp(a, b));
    }
    Ok(groups)
}

fn put_number(out: &mut Vec<u8>, num: &str, flagged: bool) {
    let (flag, digits) = if let Some(d) = num.strip_prefix("SP") {
        (2u8, d)
    } else if let Some(d) = num.strip_prefix('R') {
        (1u8, d)
    } else {
        (0u8, num)
    };
    if flagged {
        out.push(flag);
    }
    put_varint(out, digits.parse().unwrap_or(0));
}

fn read_number(r: &mut Reader, flagged: bool) -> Result<String, String> {
    let flag = if flagged { r.byte()? } else { 0 };
    let n = r.varint()?;
    Ok(match flag {
        0 => format!("{n:03}"),
        1 => format!("R{n:02}"),
        2 => format!("SP{n}"),
        f => return Err(format!("Unknown number-prefix flag: {f}")),
    })
}

fn encode_fixed(out: &mut Vec<u8>, deck: &[CodeCard], max: u32, flagged: bool) -> Result<(), String> {
    for count in (1..=max).rev() {
        let cards: Vec<&CodeCard> = deck.iter().filter(|c| c.count == count).collect();
        let groups = group(&cards)?;
        put_varint(out, groups.len() as u32);
        for g in groups {
            put_varint(out, g.nums.len() as u32);
            out.push(g.set);
            out.push(g.var);
            for n in &g.nums {
                put_number(out, n, flagged);
            }
        }
    }
    Ok(())
}

fn encode_sparse(out: &mut Vec<u8>, deck: &[CodeCard], flagged: bool) -> Result<(), String> {
    let mut counts: Vec<u32> = deck.iter().map(|c| c.count).filter(|&c| c >= 1).collect();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    counts.dedup();
    put_varint(out, counts.len() as u32);
    for count in counts {
        put_varint(out, count);
        let cards: Vec<&CodeCard> = deck.iter().filter(|c| c.count == count).collect();
        let groups = group(&cards)?;
        put_varint(out, groups.len() as u32);
        for g in groups {
            put_varint(out, g.nums.len() as u32);
            out.push(g.set);
            out.push(g.var);
            for n in &g.nums {
                put_number(out, n, flagged);
            }
        }
    }
    Ok(())
}

fn decode_group_cards(r: &mut Reader, count: u32, flagged: bool, out: &mut Vec<CodeCard>) -> Result<(), String> {
    let n = r.varint()?;
    let set = set_name(r.byte()?)?;
    let var = var_name(r.byte()?)?;
    for _ in 0..n {
        let num = read_number(r, flagged)?;
        out.push(CodeCard { code: format!("{set}-{num}{var}"), count });
    }
    Ok(())
}

fn put_card_ref(out: &mut Vec<u8>, code: &str, flagged: bool) -> Result<(), String> {
    let p = parse_code(code)?;
    out.push(set_id(&p.set)?);
    out.push(var_id(&p.variant)?);
    put_number(out, &p.number, flagged);
    Ok(())
}

fn read_card_ref(r: &mut Reader, flagged: bool) -> Result<String, String> {
    let set = set_name(r.byte()?)?;
    let var = var_name(r.byte()?)?;
    let num = read_number(r, flagged)?;
    Ok(format!("{set}-{num}{var}"))
}

pub fn encode(deck: &DecodedDeck) -> Result<String, String> {
    for c in deck.main.iter().chain(deck.side.iter()) {
        if c.count < 1 {
            return Err(format!("Invalid card count for {}", c.code));
        }
    }
    let all_codes: Vec<&str> = deck
        .main
        .iter()
        .chain(deck.side.iter())
        .map(|c| c.code.as_str())
        .chain(deck.champion.iter().map(|s| s.as_str()))
        .chain(deck.legends.iter().map(|s| s.as_str()))
        .collect();
    let mut needs_v4 = false;
    let mut any_sp = false;
    for c in &all_codes {
        let p = parse_code(c)?;
        needs_v4 |= p.number.starts_with('R');
        any_sp |= p.number.starts_with("SP");
    }
    let max_main = deck.main.iter().map(|c| c.count).max().unwrap_or(0);
    let max_side = deck.side.iter().map(|c| c.count).max().unwrap_or(0);
    let needs_v5 = max_main > 12 || max_side > 3;
    let version: u8 = if !deck.legends.is_empty() {
        6
    } else if needs_v5 || any_sp {
        5
    } else if needs_v4 {
        4
    } else {
        3
    };
    let flagged = needs_v4 || any_sp;
    let mut out = vec![(FORMAT << 4) | version];
    if version >= 5 {
        out.push(flagged as u8);
        encode_sparse(&mut out, &deck.main, flagged)?;
        encode_sparse(&mut out, &deck.side, flagged)?;
    } else {
        encode_fixed(&mut out, &deck.main, 12, version >= 4)?;
        encode_fixed(&mut out, &deck.side, 3, version >= 4)?;
    }
    match &deck.champion {
        Some(c) => {
            out.push(1);
            put_card_ref(&mut out, c, flagged)?;
        }
        None => out.push(0),
    }
    if version >= 6 {
        put_varint(&mut out, deck.legends.len() as u32);
        for l in &deck.legends {
            put_card_ref(&mut out, l, flagged)?;
        }
    }
    Ok(b32_encode(&out))
}

pub fn decode(code: &str) -> Result<DecodedDeck, String> {
    let bytes = b32_decode(code)?;
    let mut r = Reader { b: &bytes, i: 0 };
    let fv = r.byte()?;
    let (format, version) = (fv >> 4, fv & 0x0f);
    if format != FORMAT {
        return Err(format!("Unsupported format: {format}"));
    }
    if version > MAX_VERSION || version == 0 {
        return Err(format!("Unsupported version: {version}"));
    }
    let flagged = if version >= 5 {
        let f = r.byte()?;
        if f > 1 {
            return Err(format!("Unsupported deck prefix flag: {f}"));
        }
        f == 1
    } else {
        version >= 4
    };
    let mut d = DecodedDeck::default();
    if version >= 5 {
        for target in 0..2 {
            let n_counts = r.varint()?;
            for _ in 0..n_counts {
                let count = r.varint()?;
                let groups = r.varint()?;
                for _ in 0..groups {
                    let out = if target == 0 { &mut d.main } else { &mut d.side };
                    decode_group_cards(&mut r, count, flagged, out)?;
                }
            }
        }
    } else {
        for count in (1..=12).rev() {
            for _ in 0..r.varint()? {
                decode_group_cards(&mut r, count, flagged, &mut d.main)?;
            }
        }
        if version >= 2 {
            for count in (1..=3).rev() {
                for _ in 0..r.varint()? {
                    decode_group_cards(&mut r, count, flagged, &mut d.side)?;
                }
            }
        }
    }
    if version >= 3 && r.byte()? == 1 {
        d.champion = Some(read_card_ref(&mut r, flagged)?);
    }
    if version >= 6 {
        for _ in 0..r.varint()? {
            d.legends.push(read_card_ref(&mut r, flagged)?);
        }
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(code: &str, count: u32) -> CodeCard {
        CodeCard { code: code.into(), count }
    }

    #[test]
    fn readme_example_decodes() {
        // Kai'Sa deck from the Piltover Archive README.
        let d = decode("CIAAAAAAAAAQCAAAA4AACAIAABMQAAILAAAAICIMDMOVOX3AM5UHIAIDAAACO6XYAEAQKAAABX3QDGACUABKIAQAAEBQAAAWDBOQCAQAABMHE").unwrap();
        let total: u32 = d.main.iter().map(|c| c.count).sum();
        assert!(total >= 50, "total {total}");
        assert!(!d.side.is_empty());
        // round trip
        let again = decode(&encode(&d).unwrap()).unwrap();
        let mut a = d.main.clone();
        let mut b = again.main.clone();
        a.sort_by(|x, y| x.code.cmp(&y.code));
        b.sort_by(|x, y| x.code.cmp(&y.code));
        assert_eq!(a, b);
    }

    #[test]
    fn roundtrip_all_versions() {
        let base = DecodedDeck { main: vec![cc("OGN-251", 1), cc("OGN-001", 3), cc("OGN-007", 6), cc("SFD-120a", 2)], side: vec![cc("OGN-010", 2)], champion: Some("OGN-030".into()), legends: vec![] };
        assert_eq!(decode(&encode(&base).unwrap()).unwrap().champion, base.champion);
        let mut v4 = base.clone();
        v4.main.push(cc("VEN-R01", 5));
        let got = decode(&encode(&v4).unwrap()).unwrap();
        assert!(got.main.contains(&cc("VEN-R01", 5)));
        let mut v5 = base.clone();
        v5.main.push(cc("VEN-SP1", 20));
        let got = decode(&encode(&v5).unwrap()).unwrap();
        assert!(got.main.contains(&cc("VEN-SP1", 20)));
        let mut v6 = base.clone();
        v6.legends = vec!["OGN-249".into(), "OGN-247".into()];
        assert_eq!(decode(&encode(&v6).unwrap()).unwrap().legends, v6.legends);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("!!!").is_err());
        assert!(decode("").is_err());
        assert!(decode("AAAA").is_err());
    }
}
