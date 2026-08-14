//! vsms integration example (Rust): the full HTTP path a third-party
//! backend uses to send one message through vsms — no in-process
//! shortcuts, no `Procedures::send_message` call, nothing this crate
//! could not also do from a different repository entirely.
//!
//! Everything about the `private_key_jwt` token exchange (sign an RFC
//! 7523 §3 assertion, exchange it at `POST {issuer}/token`, cache and
//! attach the resulting Bearer token, refresh once on an unexpected 401)
//! now lives in `vsms-sdk-rust` (#171) rather than here — this file used
//! to hand-roll all of that in ~230 lines; see the git history of this
//! file for what that looked like. `--idempotency-key`'s `Idempotency-Key`
//! header attachment and `Idempotency-Replayed` echo (#153/#161) moved
//! into the SDK too, once the SDK existed to move it into — see
//! `VsmsClient::send_message`'s own doc for why that needed a lower-level
//! call than the generated client's typed procedure method. What's left
//! is the part an integrator actually cares about: build a
//! `SendMessageInput`, call `client.send_message(...)`, read the result
//! back.
//!
//! ```bash
//! cargo run -- \
//!   --issuer http://127.0.0.1:8080 \
//!   --client-id <clientId that provision-client printed> \
//!   --private-key-path /path/to/console-client-key.pem \
//!   --to +237677123456 \
//!   --sender-id VYMALO \
//!   --body "Hello from the vsms Rust example"
//! ```
//!
//! Every flag also reads from an env var (`VSMS_ISSUER`,
//! `VSMS_CLIENT_ID`, `VSMS_PRIVATE_KEY_PATH`, `VSMS_SCOPE`) so a real
//! integration never has to hardcode a credential path in argv.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{PrivateKeyJwtConfig, VsmsClient};

#[derive(Parser, Debug)]
#[command(
    about = "vsms integration example: private_key_jwt token exchange + sendMessage over real HTTP, via vsms-sdk-rust"
)]
struct Cli {
    /// The gateway's externally reachable origin — both the OIDC issuer
    /// and `/token` hang off this. `just demo` serves it at
    /// http://127.0.0.1:8080 by default.
    #[arg(long, env = "VSMS_ISSUER", default_value = "http://127.0.0.1:8080")]
    issuer: String,

    /// The client id `sms-gateway provision-client` printed as
    /// `provisioned client: <id>`.
    #[arg(long, env = "VSMS_CLIENT_ID")]
    client_id: String,

    /// Path to the PEM private key `sms-gateway provision-client` wrote
    /// via `--key-out`. Never commit this file, and never pass its
    /// *contents* on the command line — only the path.
    #[arg(long, env = "VSMS_PRIVATE_KEY_PATH")]
    private_key_path: PathBuf,

    /// Space-separated scopes to request. Must be a subset of what the
    /// client was provisioned with (`sms-gateway provision-client
    /// --scope ...`), or `/token` mints a narrower-than-requested token.
    #[arg(long, env = "VSMS_SCOPE", default_value = "sms:send sms:read")]
    scope: String,

    /// E.164 recipient.
    #[arg(long)]
    to: String,

    /// A sender id already `approved` for this deployment's Orange
    /// account (see the main repo's `send_test_message` example, or
    /// `just demo`'s own seeding, for what "approved" requires here).
    #[arg(long)]
    sender_id: String,

    #[arg(long, default_value = "Hello from the vsms Rust integration example")]
    body: String,

    /// A caller-chosen dedupe key, forwarded as `sendMessage`'s
    /// `clientRef`. Optional — but pass the *same* value across two runs
    /// and the second is rejected as `409 Conflict` instead of sending a
    /// second real SMS. This is the DB-level defence (`messages_app_idem_key`)
    /// — see `--idempotency-key` below for the HTTP-level one, and this
    /// crate's own README for how the two differ.
    #[arg(long)]
    client_ref: Option<String>,

    /// Sent as the `Idempotency-Key` request header on the `sendMessage`
    /// call — vsms's own `IdempotencyLayer` (#153,
    /// `backends/crates/sms-api/src/router.rs`). Optional — but pass the *same*
    /// value across two runs within the TTL window (24h by default) and
    /// the second call never re-executes `sendMessage` at all: it replays
    /// the exact first response, `Idempotency-Replayed: true`, with no
    /// second SMS and no second `Message` row. Distinct from
    /// `--client-ref`: this key is scoped by the caller's own
    /// `Authorization` header, not by `App`, and works even when the
    /// request never reaches procedure code (a client that never learns
    /// whether its first attempt was received — a timeout, a dropped
    /// connection — is exactly the case this exists for). Reusing the
    /// same key with a *different* body/path/method returns `422
    /// idempotency_key_conflict` instead of either sending or replaying.
    #[arg(long)]
    idempotency_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = PrivateKeyJwtConfig::from_key_path(
        &cli.issuer,
        &cli.client_id,
        &cli.private_key_path,
        &cli.scope,
    )
    .with_context(|| {
        format!(
            "loading the private key at {}",
            cli.private_key_path.display()
        )
    })?;
    let client =
        VsmsClient::private_key_jwt(&cli.issuer, config).context("building the vsms client")?;

    let args = SendMessageInput {
        to: cli.to,
        body: cli.body,
        senderId: Some(cli.sender_id),
        class: None,
        clientRef: cli.client_ref,
        scheduledAt: None,
        validityMinutes: None,
    };

    // Two independent dedupe layers can both reject this call, and both
    // happen to surface as `409 Conflict` — see `SdkError::is_conflict`'s
    // own doc for why one status code covers two different mechanisms.
    // Check the more specific one (`Idempotency-Key` still in flight)
    // before falling back to the generic 409 (`clientRef`'s database-level
    // dedupe), so the message printed always names the *right* layer
    // rather than guessing.
    let outcome = match client
        .send_message(args, cli.idempotency_key.as_deref())
        .await
    {
        Ok(outcome) => outcome,
        Err(error) if error.is_idempotency_in_flight() => {
            bail!(
                "sendMessage returned 409 Conflict — another request under this \
                 --idempotency-key is still being processed by IdempotencyLayer (reserved but \
                 not yet replayable). This is a genuinely concurrent duplicate call, not \
                 clientRef's dedupe below — retry shortly, once the in-flight request \
                 finishes: {error}"
            );
        }
        Err(error) if error.is_idempotency_key_conflict() => {
            bail!(
                "sendMessage returned 422 — this --idempotency-key was already used with a \
                 different request (a different --to/--body/... than the first call under this \
                 key). IdempotencyLayer refuses to guess which one you meant: pass a new \
                 --idempotency-key, or repeat the exact same request to replay it: {error}"
            );
        }
        Err(error) if error.is_conflict() => {
            println!(
                "sendMessage returned 409 Conflict — if --client-ref was passed, that clientRef \
                 was already used on a prior send. This is clientRef's database-level dedupe \
                 doing exactly its job, not a bug to retry around: {error}"
            );
            return Ok(());
        }
        Err(error) => return Err(error).context("calling sendMessage"),
    };

    if outcome.idempotency_replayed {
        println!(
            "Idempotency-Replayed: true — this is the cached response from the first call \
             under this --idempotency-key, not a new send"
        );
    }
    let result = outcome.result;

    println!();
    println!(
        "sent: messageId={} state={} encoding={} segments={} operator={} estimatedCostXaf={}",
        result.messageId,
        result.state,
        result.encoding,
        result.segments,
        result.operator,
        result.estimatedCostXaf
    );

    // Prove the write actually landed — read it back through the REST
    // surface rather than trusting the mutation's own echoed response.
    let message = client
        .get_message(&result.messageId)
        .await
        .context("calling GET /messages/{id}")?;
    println!(
        "read back: id={} state={} providerMessageRef={:?}",
        message.id, message.state, message.providerMessageRef
    );

    Ok(())
}
