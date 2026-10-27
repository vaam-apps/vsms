//! What the API refuses to store on a `WebhookEndpoint`, with no database: the
//! generated create and update inputs' own `validate()`, which every write
//! through the generated routes and delegates runs first.
//!
//! - **A sealed endpoint's URL must be one a delivery can be sealed for.** The
//!   worker binds the path of the endpoint's URL as the seal's route and refuses
//!   a URL with credentials, a query, a fragment, a `{`, `}` or `%` in the path,
//!   or a scheme other than http(s) (`sms_webhook::delivery_route`). A sealed
//!   URL stored anyway could only fail at delivery, burn eight attempts and trip
//!   the endpoint's breaker, so the database refuses it (a CHECK that applies to
//!   `sealed` rows alone, `sealed_webhooks_migration_live_postgres.rs`) and the
//!   mode-switching procedure refuses it first. An **`hmac`** endpoint binds no
//!   route and keeps any URL (`url` is a plain `@uri`), a query string included.
//!   The CHECK's pattern is `sms_webhook::SEALABLE_URL_PATTERN`: narrower than
//!   `delivery_route` in the text it reads (a host is ASCII, a port is
//!   1..=65535) and wider where only a parser can tell (`[:]`, `1.2.3.999`,
//!   `x.0`). What holds is that the pattern **together with `@uri`** (the
//!   `Url::parse` the generated input's `validate()` runs on every write through
//!   the API) never accepts what `delivery_route` refuses;
//!   `every_url_the_pattern_accepts_can_be_sealed` holds that over a generated
//!   corpus, with `delivery_route` as the oracle.
//! - **A secret written through the API is at least 32 characters**, current or
//!   rotated in; `prevSecret`, which only ever receives an earlier `secret`, has
//!   no bound.
//! - **The audience is never `vsms`.** That is the name the gateway's own routes
//!   answer to. The migration adds the same refusal as a CHECK
//!   (`sealed_webhooks_migration_live_postgres.rs` proves the database says no
//!   too).
//!
//! Plain `cargo test`: no Docker, no Postgres.

use sms_api::schema::models::WebhookEndpoint;
use sms_api::schema::{
    CreateWebhookEndpointInput, UpdateWebhookEndpointInput, WebhookDeliveryMode,
};
use sms_webhook::{delivery_route, url_can_be_sealed};

use cratestack::{CreateModelInput, UpdateModelInput};

fn create(url: &str, audience: &str) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        appId: "capp00000000000000000".to_owned(),
        url: url.to_owned(),
        deliveryMode: WebhookDeliveryMode::sealed,
        audience: Some(audience.to_owned()),
        secret: None,
        prevSecret: None,
        secretRotatedAt: None,
        eventTypes: " message.delivered ".to_owned(),
        maskRecipient: true,
        maxAttempts: 8,
        circuitOpenUntil: None,
    }
}

/// A secret of a length the API takes, shaped like the ones it mints.
const SECRET: &str = "whsec_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn hmac_create(url: &str) -> CreateWebhookEndpointInput {
    CreateWebhookEndpointInput {
        deliveryMode: WebhookDeliveryMode::hmac,
        audience: None,
        secret: Some(SECRET.to_owned()),
        ..create(url, "unused")
    }
}

/// Whether the generated create input takes `url` for an `hmac` endpoint: it
/// holds only `@uri`, so a query string, credentials or a `%` all pass.
fn accepts_hmac_url(url: &str) -> bool {
    <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&hmac_create(url))
        .is_ok()
}

/// Whether the generated create input's `@uri` (`url::Url::parse`) takes `url`.
fn accepts_uri(url: &str) -> bool {
    accepts_hmac_url(url)
}

fn accepts_audience(audience: &str) -> bool {
    <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&create(
        "https://api.vaam.store/webhooks/vsms",
        audience,
    ))
    .is_ok()
}

#[test]
fn a_url_a_delivery_can_be_sealed_for_matches_the_pattern() {
    for url in [
        "https://api.vaam.store/webhooks/vsms",
        "http://127.0.0.1:8080/webhooks/vsms",
        "HTTPS://Example.COM/hook",
        "https://example.com",
        "https://example.com/",
        "https://example.com:65535/a/b-c_d.e~f",
        "https://[::1]:8443/hook",
        "https://sub.domain.example.co.uk/a/b/c/",
        "https://example.com/a,b;c=d@e:f",
    ] {
        assert!(url_can_be_sealed(url), "{url} should match");
        assert!(delivery_route(url).is_ok(), "{url} should be sealable");
    }
}

#[test]
fn a_url_that_cannot_be_sealed_does_not_match_the_pattern() {
    for url in [
        // Credentials, a query, a fragment (empty ones included: `?` and `#`
        // are refused whatever follows them).
        "https://user:pw@example.com/hook",
        "https://user@example.com/hook",
        "https://example.com/hook?x=1",
        "https://example.com/hook?",
        "https://example.com/hook#frag",
        "https://example.com/hook#",
        // A path that is not one literal route.
        "https://example.com/hooks/{tenant}",
        "https://example.com/a%2Fb",
        "https://example.com/a}",
        // A scheme the delivery cannot use, and things that are not URLs.
        "ftp://example.com/hook",
        "file:///etc/passwd",
        "not a url",
        "",
        "https://",
        "https:///hook",
        "https://example.com/ho ok",
        "https://example.com/hook\n",
        // A host this pattern does not write (an IDN is written in punycode),
        // and a port out of range or zero.
        "https://exämple.com/hook",
        "https://under_score.example.com/hook",
        "https://example.com:99999/hook",
        "https://example.com:0/hook",
        "https://example.com:/hook",
    ] {
        assert!(!url_can_be_sealed(url), "{url:?} should not match");
    }
}

/// The rule is a sealed endpoint's alone. Every URL above is a URL an `hmac`
/// endpoint may hold, as it always could: the generated input checks `@uri` and
/// nothing about a route.
#[test]
fn an_hmac_endpoint_keeps_a_url_a_sealed_one_could_not_have() {
    for url in [
        "https://example.com/hook?token=abc123",
        "https://example.com/hook?",
        "https://example.com/hook#frag",
        "https://user:pw@example.com/hook",
        "https://example.com/hooks/{tenant}",
        "https://example.com/a%2Fb",
        "https://under_score.example.com/hook",
        "https://api.vaam.store/webhooks/vsms",
    ] {
        assert!(accepts_hmac_url(url), "{url} should be accepted for hmac");
    }
}

/// Every combination of the pieces a URL is made of, good and bad: whenever the
/// pattern **and** `@uri` (the generated input's `validate()`) would let a sealed
/// endpoint keep one, the worker can seal for it. The pattern alone is not enough:
/// it is a pattern, not a parser, and lets `[:]`, `1.2.3.999` and `x.0` through,
/// which `Url::parse` refuses; the corpus holds those and the test below proves
/// they are the ones `@uri` is for.
#[test]
fn every_url_the_pattern_accepts_can_be_sealed() {
    let schemes = ["https://", "http://", "HTTP://", "ftp://", "ws://", ""];
    let userinfo = ["", "u@", "u:p@", "@"];
    let hosts = [
        "example.com",
        "a-b.example.co",
        "127.0.0.1",
        "[::1]",
        "[:]",
        "[1:2]",
        "1.2.3.999",
        "x.0",
        "example.0",
        "-bad.example",
        "bad-.example",
        "ex ample.com",
        "exämple.com",
        "under_score.example",
        "",
        "h{x}.com",
    ];
    let ports = ["", ":8080", ":65535", ":65536", ":0", ":", ":abc", ":080"];
    let paths = [
        "", "/", "/hook", "/a/b/c", "/{x}", "/a%20b", "/a b", "/é", "//double", "/a/../b",
        "/a;b=c,d",
    ];
    let tails = ["", "?", "?x=1", "#", "#f", "?x#f"];
    let mut accepted = 0_usize;
    let mut accepted_by_both = 0_usize;
    let mut parse_only_refusals = 0_usize;
    let mut total = 0_usize;
    for scheme in schemes {
        for user in userinfo {
            for host in hosts {
                for port in ports {
                    for path in paths {
                        for tail in tails {
                            let url = format!("{scheme}{user}{host}{port}{path}{tail}");
                            total += 1;
                            if !url_can_be_sealed(&url) {
                                continue;
                            }
                            accepted += 1;
                            if accepts_uri(&url) {
                                accepted_by_both += 1;
                                assert!(
                                    delivery_route(&url).is_ok(),
                                    "the pattern and @uri accept {url:?} but delivery_route \
                                     refuses it: {:?}",
                                    delivery_route(&url)
                                );
                            } else if delivery_route(&url).is_ok() {
                                panic!(
                                    "@uri refuses {url:?}, which the pattern accepts and \
                                     delivery_route can seal: the two parsers disagree"
                                );
                            } else {
                                parse_only_refusals += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    // The corpus is not vacuous: it holds both kinds.
    assert!(total > 100_000, "{total}");
    assert!(accepted > 100, "only {accepted} of {total} accepted");
    assert!(accepted < total / 2, "{accepted} of {total} accepted");
    // ... and it holds the shapes only a parser refuses, so the assertion above
    // is about the pattern together with @uri and not the pattern alone.
    assert!(accepted_by_both > 100, "{accepted_by_both}");
    assert!(
        parse_only_refusals > 100,
        "the corpus must contain URLs the pattern accepts and @uri refuses: {parse_only_refusals}"
    );
}

/// The shapes the pattern lets through and `@uri` refuses, named: this is why the
/// schema keeps `@uri` on a sealed `url`, and why the console asks the parser too.
#[test]
fn the_pattern_alone_accepts_what_a_parser_refuses() {
    for url in [
        "https://[:]/hook",
        "https://[1:2]/hook",
        "https://1.2.3.999/hook",
        "https://999.999.999.999/hook",
        "https://x.0/hook",
        "https://example.0/hook",
    ] {
        assert!(url_can_be_sealed(url), "{url}: the pattern is not a parser");
        assert!(!accepts_uri(url), "{url}: @uri refuses it");
        assert!(delivery_route(url).is_err(), "{url}: so does the worker");
    }
}

#[test]
fn an_audience_is_never_the_name_the_gateways_own_routes_answer_to() {
    assert!(!accepts_audience("vsms"));
    for fine in [
        "vaam",
        "vsms-receiver",
        "Vsms",
        "VSMS",
        "myvsms",
        "vsms ",
        " vsms",
        "vsm",
        "vs",
        "v",
        "https://hooks.example.test:8443",
    ] {
        assert!(accepts_audience(fine), "{fine:?} is another name");
    }
    // The other bounds `@length` already gave it.
    assert!(!accepts_audience(""));
    assert!(!accepts_audience(&"a".repeat(256)));
    assert!(accepts_audience(&"a".repeat(255)));
}

#[test]
fn an_update_is_held_to_the_audience_rules_and_the_secret_bound() {
    let validate = |input: UpdateWebhookEndpointInput| {
        <UpdateWebhookEndpointInput as UpdateModelInput<WebhookEndpoint>>::validate(&input).is_ok()
    };
    let update = || UpdateWebhookEndpointInput::default();
    assert!(validate(update()));
    assert!(validate(UpdateWebhookEndpointInput {
        url: Some("https://api.vaam.store/webhooks/vsms".to_owned()),
        audience: Some(Some("vaam".to_owned())),
        ..update()
    }));
    assert!(!validate(UpdateWebhookEndpointInput {
        audience: Some(Some("vsms".to_owned())),
        ..update()
    }));
    // The URL is not held to the route rule here: the row's mode decides that
    // (a CHECK), and an update does not carry it. A legacy `hmac` endpoint's URL
    // with a query string saves as it was.
    assert!(validate(UpdateWebhookEndpointInput {
        url: Some("https://api.vaam.store/webhooks/vsms?token=1".to_owned()),
        ..update()
    }));
}

/// A secret the API takes is at least 32 characters, whichever way it arrives;
/// a rotation can still move any earlier secret into `prevSecret`; and a
/// `null` clears either, as a switch to `sealed` must.
#[test]
fn a_secret_is_at_least_32_characters_and_a_previous_one_has_no_bound() {
    let create_ok = |secret: Option<&str>| {
        let input = CreateWebhookEndpointInput {
            secret: secret.map(str::to_owned),
            ..hmac_create("https://api.vaam.store/webhooks/vsms")
        };
        <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&input).is_ok()
    };
    assert!(create_ok(Some(SECRET)));
    assert!(create_ok(Some(&"s".repeat(32))));
    assert!(!create_ok(Some(&"s".repeat(31))));
    assert!(!create_ok(Some("")));
    assert!(
        create_ok(None),
        "absent is a CHECK's business, not this bound's"
    );

    let validate = |input: UpdateWebhookEndpointInput| {
        <UpdateWebhookEndpointInput as UpdateModelInput<WebhookEndpoint>>::validate(&input).is_ok()
    };
    let update = || UpdateWebhookEndpointInput::default();
    assert!(validate(UpdateWebhookEndpointInput {
        secret: Some(Some(SECRET.to_owned())),
        ..update()
    }));
    assert!(!validate(UpdateWebhookEndpointInput {
        secret: Some(Some("short".to_owned())),
        ..update()
    }));
    assert!(!validate(UpdateWebhookEndpointInput {
        secret: Some(Some(String::new())),
        ..update()
    }));
    assert!(validate(UpdateWebhookEndpointInput {
        secret: Some(None),
        prevSecret: Some(None),
        ..update()
    }));
    // The previous secret is whatever `secret` was, short or not.
    assert!(validate(UpdateWebhookEndpointInput {
        secret: Some(Some(SECRET.to_owned())),
        prevSecret: Some(Some("an-older-and-shorter-secret".to_owned())),
        ..update()
    }));
}

/// An `hmac` endpoint has no audience, so the audience rules must not demand
/// one: the field is optional, and `null` is a valid value for it.
#[test]
fn an_hmac_endpoint_needs_no_audience() {
    let input = hmac_create("https://api.vaam.store/webhooks/vsms");
    assert!(input.audience.is_none());
    assert!(
        <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&input).is_ok()
    );
}

/// The wire shape the console switches an endpoint's mode with: an explicit
/// `null` clears a nullable column (`Some(None)`), an omitted key leaves it
/// alone (`None`). Without that distinction a `sealed` switch could not remove
/// the secrets, and the CHECK would refuse the write.
#[test]
fn a_json_null_clears_a_nullable_column_and_an_omitted_key_leaves_it() {
    let cleared: UpdateWebhookEndpointInput = serde_json::from_str(
        r#"{"deliveryMode":"sealed","audience":"vaam","secret":null,"prevSecret":null,"secretRotatedAt":null}"#,
    )
    .unwrap();
    assert_eq!(cleared.deliveryMode, Some(WebhookDeliveryMode::sealed));
    assert_eq!(cleared.audience, Some(Some("vaam".to_owned())));
    assert_eq!(cleared.secret, Some(None));
    assert_eq!(cleared.prevSecret, Some(None));
    assert_eq!(cleared.secretRotatedAt, Some(None));

    let untouched: UpdateWebhookEndpointInput =
        serde_json::from_str(r#"{"active":false}"#).unwrap();
    assert_eq!(untouched.secret, None);
    assert_eq!(untouched.audience, None);
    assert_eq!(untouched.deliveryMode, None);
}

/// The wire names of the two modes are the schema's variants, lower case.
#[test]
fn the_modes_are_named_sealed_and_hmac_on_the_wire() {
    assert_eq!(
        serde_json::to_string(&WebhookDeliveryMode::sealed).unwrap(),
        r#""sealed""#
    );
    assert_eq!(
        serde_json::to_string(&WebhookDeliveryMode::hmac).unwrap(),
        r#""hmac""#
    );
    assert!(serde_json::from_str::<WebhookDeliveryMode>(r#""plain""#).is_err());
}
