//! Cross-checks the Rust deck code implementation against fixtures produced by
//! the reference JS library (e2e/gen_fixtures.cjs).

use rift_shared::deckcode::{decode, encode, CodeCard, DecodedDeck};
use serde_json::Value;

fn cards(v: &Value) -> Vec<CodeCard> {
    v.as_array().unwrap().iter().map(|c| CodeCard { code: c["cardCode"].as_str().unwrap().into(), count: c["count"].as_u64().unwrap() as u32 }).collect()
}

fn sorted(mut v: Vec<CodeCard>) -> Vec<CodeCard> {
    v.sort_by(|a, b| a.code.cmp(&b.code));
    v
}

#[test]
fn matches_reference_library() {
    let fx: Vec<Value> = serde_json::from_str(include_str!("deckcode_fixtures.json")).unwrap();
    assert!(fx.len() >= 5);
    for f in fx {
        let expect = DecodedDeck {
            main: cards(&f["main"]),
            side: cards(&f["side"]),
            champion: f["champ"].as_str().map(|s| s.to_string()),
            legends: f["legends"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect(),
        };
        let code = f["code"].as_str().unwrap();
        // Encoding must be byte-identical to the JS output.
        assert_eq!(encode(&expect).unwrap(), code, "encode mismatch for {code}");
        let got = decode(code).unwrap();
        assert_eq!(sorted(got.main), sorted(expect.main.clone()));
        assert_eq!(sorted(got.side), sorted(expect.side.clone()));
        assert_eq!(got.champion, expect.champion);
        assert_eq!(got.legends, expect.legends);
    }
}
