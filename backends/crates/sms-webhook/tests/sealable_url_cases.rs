//! `SEALABLE_URL_PATTERN` against the corpus its TypeScript twin
//! (`frontends/packages/gateway/src/webhook-rules.ts`, which reads this same
//! file) is held to as well: one URL, one answer, in both languages. The
//! console's early check is a copy of the database's CHECK, and a rule written
//! twice agrees only while something compares the two.
//!
//! The corpus separates the two things a `sealed` row has to satisfy. `pattern`
//! is the CHECK alone, which accepts a few shapes that URL parsing refuses (an
//! IPv6 literal with too few groups, an IPv4-shaped host with an octet out of
//! range, a numeric last label); `sealable` is the pattern and `delivery_route`
//! together, which is what the worker needs and the console says.

use serde_json::Value;
use sms_webhook::{delivery_route, url_can_be_sealed};

fn corpus() -> Vec<Value> {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sealable_url_cases.json"),
    )
    .unwrap();
    let table: Value = serde_json::from_str(&text).unwrap();
    table["cases"].as_array().unwrap().clone()
}

#[test]
fn the_pattern_gives_every_url_in_the_shared_corpus_its_recorded_answer() {
    let cases = corpus();
    assert!(cases.len() > 40, "{}", cases.len());
    for case in &cases {
        let url = case["url"].as_str().unwrap();
        assert_eq!(
            url_can_be_sealed(url),
            case["pattern"].as_bool().unwrap(),
            "the pattern on {url:?}: {}",
            case["why"]
        );
    }
}

#[test]
fn the_pattern_and_the_route_together_give_every_url_its_recorded_sealability() {
    for case in &corpus() {
        let url = case["url"].as_str().unwrap();
        let sealable = url_can_be_sealed(url) && delivery_route(url).is_ok();
        assert_eq!(
            sealable,
            case["sealable"].as_bool().unwrap(),
            "the pattern and delivery_route on {url:?}: {}",
            case["why"]
        );
    }
}

#[test]
fn the_corpus_is_not_all_one_answer() {
    let cases = corpus();
    let count = |key: &str, want: bool| {
        cases
            .iter()
            .filter(|case| case[key].as_bool().unwrap() == want)
            .count()
    };
    assert!(count("pattern", true) > 10 && count("pattern", false) > 20);
    assert!(
        count("sealable", false) > count("pattern", false),
        "some rows are accepted by the pattern and refused by URL parsing"
    );
    assert!(cases.iter().any(|case| {
        case["pattern"].as_bool().unwrap() && !case["sealable"].as_bool().unwrap()
    }));
}
