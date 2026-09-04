# vsms-sdk-rust

A Rust client for [vsms](https://github.com/vaam-apps/vsms) that owns the
`private_key_jwt` credential lifecycle, so a caller writes
`client.send_message(...)` and never touches a JWT.

See [issue #171](https://github.com/vymalo/vsms/issues/171) for the full
brief and `src/lib.rs`'s module doc for what is generated
(`cratestack::include_client_schema!` — models, inputs, procedure stubs)
versus hand-written (the auth layer: `TokenStore`, `PrivateKeyJwtTokenStore`,
`GatewayAuthorizer`, and `VsmsClient`'s bounded-refresh-on-401).

## Usage

```rust
use vsms_sdk::{PrivateKeyJwtConfig, VsmsClient};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = PrivateKeyJwtConfig::from_key_path(
        "http://127.0.0.1:8080",
        "the client id provision-client printed",
        "/path/to/console-client-key.pem",
        "sms:send sms:read",
    )?;
    let client = VsmsClient::private_key_jwt("http://127.0.0.1:8080", config)?;

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
            // Optional `Idempotency-Key` — a retry under the same key
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

See `examples/rust/sms-send` in the main vsms repo for a complete, runnable
version of this against a real gateway (`just demo`).

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

Not in scope, and unlikely to ever be: request signing — vsms dropped it
in favour of `private_key_jwt` (see the design doc's §4).
