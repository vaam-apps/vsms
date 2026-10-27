//! What the API refuses to store on a `WebhookEndpoint`, with no database: the
//! generated create and update inputs' own `validate()`, which every write
//! through the generated routes and delegates runs first.
//!
//! - **The URL must be one a delivery can be sealed for.** The worker binds the
//!   path of the endpoint's URL as the seal's route and refuses a URL with
//!   credentials, a query, a fragment, a `{`, `}` or `%` in the path, or a scheme
//!   other than http(s) (`sms_webhook::delivery_route`). A URL stored anyway
//!   could only fail at delivery, burn eight attempts and trip the endpoint's
//!   breaker, so the schema's `@regex` refuses it on write. The pattern is
//!   narrower than `delivery_route` (a host is ASCII, a port is 1..=65535), never
//!   wider; `every_url_the_schema_accepts_can_be_sealed` holds that over a
//!   generated corpus, with `delivery_route` as the oracle.
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
use sms_webhook::delivery_route;

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

fn accepts_url(url: &str) -> bool {
    <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&create(
        url, "vaam",
    ))
    .is_ok()
}

fn accepts_audience(audience: &str) -> bool {
    <CreateWebhookEndpointInput as CreateModelInput<WebhookEndpoint>>::validate(&create(
        "https://api.vaam.store/webhooks/vsms",
        audience,
    ))
    .is_ok()
}

#[test]
fn a_url_a_delivery_can_be_sealed_for_is_accepted() {
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
        assert!(accepts_url(url), "{url} should be accepted");
        assert!(delivery_route(url).is_ok(), "{url} should be sealable");
    }
}

#[test]
fn a_url_that_cannot_be_sealed_is_refused_on_write() {
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
        "https://example.com:99999/hook",
        "https://example.com:0/hook",
        "https://example.com:/hook",
    ] {
        assert!(!accepts_url(url), "{url:?} should be refused on write");
    }
}

/// Every combination of the pieces a URL is made of, good and bad: whenever the
/// API would store one, the worker can seal for it.
#[test]
fn every_url_the_schema_accepts_can_be_sealed() {
    let schemes = ["https://", "http://", "HTTP://", "ftp://", "ws://", ""];
    let userinfo = ["", "u@", "u:p@", "@"];
    let hosts = [
        "example.com",
        "a-b.example.co",
        "127.0.0.1",
        "[::1]",
        "-bad.example",
        "bad-.example",
        "ex ample.com",
        "exämple.com",
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
    let mut total = 0_usize;
    for scheme in schemes {
        for user in userinfo {
            for host in hosts {
                for port in ports {
                    for path in paths {
                        for tail in tails {
                            let url = format!("{scheme}{user}{host}{port}{path}{tail}");
                            total += 1;
                            if accepts_url(&url) {
                                accepted += 1;
                                assert!(
                                    delivery_route(&url).is_ok(),
                                    "the API stores {url:?} but delivery_route refuses it: \
                                     {:?}",
                                    delivery_route(&url)
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    // The corpus is not vacuous: it holds both kinds.
    assert!(total > 10_000, "{total}");
    assert!(accepted > 100, "only {accepted} of {total} accepted");
    assert!(accepted < total / 2, "{accepted} of {total} accepted");
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
fn an_update_is_held_to_the_same_rules() {
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
        url: Some("https://api.vaam.store/webhooks/vsms?x=1".to_owned()),
        ..update()
    }));
    assert!(!validate(UpdateWebhookEndpointInput {
        url: Some("https://user:pw@api.vaam.store/webhooks/vsms".to_owned()),
        ..update()
    }));
    assert!(!validate(UpdateWebhookEndpointInput {
        audience: Some(Some("vsms".to_owned())),
        ..update()
    }));
}

/// An `hmac` endpoint has no audience, so the audience rules must not demand
/// one: the field is optional, and `null` is a valid value for it.
#[test]
fn an_hmac_endpoint_needs_no_audience() {
    let input = CreateWebhookEndpointInput {
        deliveryMode: WebhookDeliveryMode::hmac,
        audience: None,
        secret: Some("whsec_x".to_owned()),
        ..create("https://api.vaam.store/webhooks/vsms", "unused")
    };
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
