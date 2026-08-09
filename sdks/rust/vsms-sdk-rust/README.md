# vsms-sdk-rust

A Rust client for [vsms](https://github.com/vymalo/vsms) that owns the
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

    let result = client
        .send_message(vsms_sdk::schema::SendMessageInput {
            to: "+237677123456".to_owned(),
            body: "Hello from vsms-sdk-rust".to_owned(),
            senderId: Some("VYMALO".to_owned()),
            class: None,
            clientRef: None,
            scheduledAt: None,
            validityMinutes: None,
        })
        .await?;

    println!("sent: {} ({})", result.messageId, result.state);
    Ok(())
}
```

See `examples/rust/sms-send` in the main vsms repo for a complete, runnable
version of this against a real gateway (`just demo`).

## What's vendored, and why

`schema.cstack` in this directory is a plain copy of the main repo's
`schema/schema.cstack`, refreshed by `vendor-schema.sh`. It has to live
inside this crate (not be reached via a `../../schema/` path climb) so
that `include_client_schema!` still resolves once this crate is published
and built from a downstream integrator's own Cargo registry cache — see
`src/lib.rs`'s module doc for the full reasoning.

## Scope

Not in scope, and unlikely to ever be: request signing (vsms dropped it
in favour of `private_key_jwt` — see the design doc's §4) and
`Idempotency-Key` support (blocked on [#153](https://github.com/vymalo/vsms/issues/153),
which hasn't landed upstream yet).
