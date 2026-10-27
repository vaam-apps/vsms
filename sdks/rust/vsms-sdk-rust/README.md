# vsms-sdk-rust

A Rust client for [vsms](https://github.com/vaam-apps/vsms) that signs every
request with your Ed25519 key and verifies every answer against the gateway's
pinned public keys, so a caller writes `client.send_message(...)` and handles
no token, no JWT and no session.

See [issue #171](https://github.com/vymalo/vsms/issues/171) for the full
brief and `src/lib.rs`'s module doc for what is generated
(`cratestack::include_client_schema!` - models, inputs, procedure stubs)
versus hand-written (`SigningKey`, `ServerKeys`, and `VsmsClient`'s sealing of
`send_message`).

## Usage

```rust
use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Your key: `openssl genpkey -algorithm ed25519`. The private half never
    // leaves you; the operator registers the public half for your app with
    // `sms-gateway register-client` (print it with the example's
    // `--print-public-key`).
    let key = SigningKey::from_pem_file("/path/to/vsms-signing-key.pem")?;
    // The gateway's two public keys (current and next), exactly as
    // `sms-gateway server-keys` printed them. Pin both: a rotation then
    // needs no flag day.
    let server_keys = ServerKeys::parse_report(&std::fs::read_to_string("server-keys.txt")?)?;
    let client = VsmsClient::signed("http://127.0.0.1:8080", key, server_keys)?;

    let outcome = client
        .send_message(
            vsms_sdk::schema::SendMessageInput {
                to: "+237677123456".to_owned(),
                body: "Hello from vsms-sdk-rust".to_owned(),
                senderId: Some("VYMALO".to_owned()),
                class: None,
                clientRef: None,
                scheduledAt: None,
                validityMinutes: None,
            },
            // Optional `Idempotency-Key` - a retry under the same key
            // replays the first response instead of sending twice. See
            // `VsmsClient::send_message`'s own doc for how this differs
            // from `clientRef` above.
            Some("a caller-chosen retry-safe key"),
        )
        .await?;

    println!(
        "sent: {} ({}), replayed={}",
        outcome.result.messageId, outcome.result.state, outcome.idempotency_replayed
    );
    Ok(())
}
```

## How a call is made

Every request is a COSE_Sign1 envelope bound to one route, one audience and one
fresh nonce, so a captured request cannot be replayed or redirected. An answer
that is not a sealed message, or does not verify against a pinned key, is an
error whatever its status; the gateway's own refusals (an unknown, revoked or
expired key, a replayed nonce) are one coarse unsigned `401`, surfaced as
`SdkError::is_unauthorized`. The SDK never retries a refused signature and
never follows a redirect.

See `examples/rust/sms-send` in the main vsms repo for a complete, runnable
version of this against a real gateway (`just demo`).

## Receiving webhooks

vsms delivers an event as a sealed request signed with the gateway's own key (the
one that signs its answers): no webhook secret, no signature header. Pin the same
two public keys and open the body:

```rust
use vsms_sdk::{ServerKeys, WebhookOpener};

let keys = ServerKeys::parse_report(&std::fs::read_to_string("server-keys.txt")?)?;
// `audience` is the name on your WebhookEndpoint (choose one only you can have);
// `route` is the whole path of its URL; `app_id` is your App's id, required.
let opener = WebhookOpener::new(&keys, "https://hooks.example.com", "/webhooks/vsms", "capp00000000000000000")?;
let event = opener.open(&raw_request_body).await?;
println!("{} {}", event.event_type, event.id);
```

One gateway key signs every app's deliveries, so a signature and an audience say
only that vsms sent this to your name. `app_id` is compared with the `appId`
inside the signed envelope, and `WebhookOpener::builder(..).endpoint_id(..)` also
pins the endpoint: a delivery made for someone else's endpoint that happens to
name your audience is refused. There is no opener without an app id.

`open` returns `WebhookError::Unverified` for a forgery, a body sealed by a key
you did not pin, a delivery made for another app or endpoint, another audience or
route, a stale timestamp (300 s) and a replayed body, and one answer is all it
gives: reply `401`. Dedupe on `event.id`: delivery is
at-least-once and a retry is a new sealed request carrying the same id. The nonce
store is per process by default; give a multi-replica receiver one it shares with
`WebhookOpener::builder(..).with_nonce_store(..)`.

## What's vendored, and why

`schema.cstack` in this directory is a plain copy of the main repo's
`schemas/vsms.cstack`, refreshed by `cargo xtask sdk-schema-vendor` (run
from the repo root; `.xtask` was `sdks/rust/vsms-sdk-rust/vendor-schema.sh`
before the maintainer's "no bash scripts" cutover). It has to live
inside this crate (not be reached via a `../../schema/` path climb) so
that `include_client_schema!` still resolves once this crate is published
and built from a downstream integrator's own Cargo registry cache — see
`src/lib.rs`'s module doc for the full reasoning.

## Idempotency-Key vs. clientRef — two different dedupe layers

`VsmsClient::send_message`'s `idempotency_key` argument sends `Idempotency-Key`
(`IdempotencyLayer`, [#153](https://github.com/vymalo/vsms/issues/153),
landed once this SDK could actually use it — see `SendMessageOutcome`'s own
doc). It protects against **not knowing whether a previous HTTP request
landed** — a timeout, a dropped connection — by replaying the exact first
response on a retry, never re-executing `sendMessage`. `SendMessageInput.clientRef`
is a different, database-level dedupe scoped by `App`, protecting against
**deliberately** sending the same logical message twice. Both can surface
as a `409` from `send_message` — `SdkError::is_idempotency_in_flight`
and `SdkError::is_conflict` distinguish them; see either method's own doc.

## Scope

Not in scope: bearer tokens, `private_key_jwt` and any other OAuth grant. vsms
removed them (VS-3); the old `VsmsClient::private_key_jwt` is gone with no
replacement, and a gateway refuses a request that carries an `Authorization`
header.
