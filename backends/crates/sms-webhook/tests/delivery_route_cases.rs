//! `delivery_route` against the table the console's TypeScript twin
//! (`frontends/packages/gateway/src/webhook-rules.ts`'s `receiverRoute`) is held
//! to as well: one URL, one answer, in both languages. The console refuses a URL
//! the worker could not seal for before the gateway is asked, and a rule written
//! twice agrees only while something compares the two; the TypeScript test reads
//! this same file. (The gateway's own `@regex` on `WebhookEndpoint.url` is held to
//! this function by `sms-api`'s `webhook_endpoint_validation.rs`.)

use serde_json::Value;
use sms_webhook::delivery_route;

#[test]
fn every_url_in_the_shared_table_has_the_same_route_in_rust() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/delivery_route_cases.json"),
    )
    .unwrap();
    let table: Value = serde_json::from_str(&text).unwrap();
    let cases = table["cases"].as_array().unwrap();
    assert!(cases.len() > 20, "{}", cases.len());
    let mut refused = 0;
    for case in cases {
        let url = case["url"].as_str().unwrap();
        let expected = case["route"].as_str();
        let got = delivery_route(url).ok();
        assert_eq!(got.as_deref(), expected, "{url:?}");
        refused += usize::from(expected.is_none());
    }
    // The table is not all one answer.
    assert!(refused > 5 && refused < cases.len() - 5, "{refused}");
}
