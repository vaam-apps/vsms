//! vsms integration example (Rust): the full HTTP path a third-party
//! backend uses to send one message through vsms — no in-process
//! shortcuts, no `Procedures::send_message` call, nothing this crate
//! could not also do from a different repository entirely.
//!
//! There is no token exchange: every request is signed with the caller's own
//! Ed25519 key (COSE_Sign1) and every answer is verified against the gateway's
//! pinned public keys. All of that (sealing, the nonce, opening the answer, the
//! `Idempotency-Key` header and the `Idempotency-Replayed` echo, #153/#161)
//! lives in `vsms-sdk-rust` (#171, VS-6); see `VsmsClient::send_message`'s own
//! doc for why that needed a lower-level call than the generated client's typed
//! procedure method. What's left is the part an integrator actually cares
//! about: build a `SendMessageInput`, call `client.send_message(...)`, read the
//! result back.
//!
//! One-time setup, by an operator with access to the gateway:
//!
//! ```bash
//! # Your key. The private half stays with you; vsms never sees it.
//! openssl genpkey -algorithm ed25519 -out vsms-signing-key.pem
//! cargo run -- --print-public-key --signing-key-path vsms-signing-key.pem
//! # -> register that public key for your app (the operator runs this):
//! sms-gateway register-client --app <slug> --public-key <that value> --scope sms:send --scope sms:read
//! # The gateway's two public keys, to pin (also the operator's):
//! sms-gateway server-keys --envelope-signing-key-file ... --envelope-next-signing-key-file ... > server-keys.txt
//! ```
//!
//! Then:
//!
//! ```bash
//! cargo run -- \
//!   --base-url http://127.0.0.1:8080 \
//!   --signing-key-path vsms-signing-key.pem \
//!   --server-keys-path server-keys.txt \
//!   --to +237677123456 \
//!   --sender-id VYMALO \
//!   --body "Hello from the vsms Rust example"
//! ```
//!
//! Every flag also reads from an env var (`VSMS_BASE_URL`,
//! `VSMS_SIGNING_KEY_PATH`, `VSMS_SERVER_KEYS_PATH`) so a real integration never
//! has to hardcode a credential path in argv.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};

#[derive(Parser, Debug)]
#[command(
    about = "vsms integration example: a signed (COSE_Sign1) sendMessage over real HTTP, via vsms-sdk-rust"
)]
struct Cli {
    /// The gateway's externally reachable origin. `just demo` serves it at
    /// http://127.0.0.1:8080 by default.
    #[arg(long, env = "VSMS_BASE_URL", default_value = "http://127.0.0.1:8080")]
    base_url: String,

    /// Path to your Ed25519 private key (PKCS#8 PEM, as `openssl genpkey
    /// -algorithm ed25519` writes). Never commit this file, and never pass
    /// its *contents* on the command line - only the path.
    #[arg(long, env = "VSMS_SIGNING_KEY_PATH")]
    signing_key_path: PathBuf,

    /// Path to the file holding `sms-gateway server-keys`' output (`current:`
    /// and `next:` lines): the gateway's two public keys, which answers are
    /// verified against. Public, so ordinary configuration.
    #[arg(
        long,
        env = "VSMS_SERVER_KEYS_PATH",
        required_unless_present = "print_public_key"
    )]
    server_keys_path: Option<PathBuf>,

    /// Print the public half of `--signing-key-path` (unpadded base64url, the
    /// form `register-client --public-key` takes) and exit. Sends nothing.
    #[arg(long)]
    print_public_key: bool,

    /// E.164 recipient.
    #[arg(long, required_unless_present = "print_public_key")]
    to: Option<String>,

    /// A sender id already `approved` for this deployment's Orange
    /// account (see the main repo's `send_test_message` example, or
    /// `just demo`'s own seeding, for what "approved" requires here).
    #[arg(long, required_unless_present = "print_public_key")]
    sender_id: Option<String>,

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
    /// `--client-ref`: this key is scoped by the caller's own verified
    /// signing key, not by `App`, and works even when the
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

    let signing_key = SigningKey::from_pem_file(&cli.signing_key_path).with_context(|| {
        format!(
            "loading the signing key at {}",
            cli.signing_key_path.display()
        )
    })?;
    if cli.print_public_key {
        println!("{}", signing_key.public_key_b64u());
        return Ok(());
    }
    let server_keys_path = cli
        .server_keys_path
        .as_ref()
        .context("--server-keys-path is required")?;
    let report = std::fs::read_to_string(server_keys_path)
        .with_context(|| format!("reading the server keys at {}", server_keys_path.display()))?;
    let server_keys = ServerKeys::parse_report(&report).context("the pinned server keys")?;
    let client = VsmsClient::signed(&cli.base_url, signing_key, server_keys)
        .context("building the vsms client")?;

    let args = SendMessageInput {
        to: cli.to.context("--to is required")?,
        body: cli.body,
        senderId: Some(cli.sender_id.context("--sender-id is required")?),
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
