//! #160's joined integration story, ported off the old
//! `scripts/e2e-integration.sh`: an external integrator sends a message
//! over real HTTP, then the admin console's *own* credential — a second,
//! independent principal under the same `App` — reads that exact message
//! id back, until it reaches `delivered`. See that deleted script's own
//! git history for the bash-and-`openssl`-and-`curl`-and-`jq` version this
//! replaces.
//!
//! # Why this runs *inside* the compose network, not on the host
//!
//! The first cut of this tool took a `--gateway-base-url`/`--gateway-issuer`
//! split, reasoning that `vsms_sdk::VsmsClient::private_key_jwt(base_url,
//! config)` keeps "the address I connect to" and "the audience I sign
//! into my assertion" separate, so a host-native process could connect to
//! `compose.dev.yaml`'s host-published gateway port while still signing
//! an assertion whose `aud` matches the gateway's *real*, internal-DNS
//! issuer. **That was wrong, found live, not by inspection**: read
//! directly from `sdks/rust/vsms-sdk-rust/src/token.rs::PrivateKeyJwtTokenStore::new`,
//! the `/token` POST is always sent to `format!("{issuer}/token")` —
//! `base_url` only ever backs the generated REST client, never the token
//! exchange. `VsmsClient::private_key_jwt`'s own doc comment claims
//! `base_url` is used "for both the OIDC `/token` endpoint and the REST
//! API"; that's the aspiration, not what the code does — a real instance
//! of this repo's own "documentation asserts something the code does not
//! do" pattern, this time in the SDK rather than in vsms itself, flagged
//! separately rather than fixed here (`sdks/rust/vsms-sdk-rust` is out of
//! this PR's owned-files scope). Confirmed two ways: an assertion signed
//! with `aud=http://127.0.0.1:<published-port>/token` (matching the
//! *reachable* address) gets a real `401 invalid_client` from the real
//! `/token` route, and `docker compose cp`ing `--gateway-base-url`
//! separate from `--gateway-issuer` never got past the send step either
//! — the token request itself always went to `--gateway-issuer`, which a
//! host process cannot resolve (`sms-gateway` is Compose-internal DNS).
//!
//! The fix is structural, not a workaround: this binary is built into its
//! own small image (`ci/e2e-integration/Dockerfile`, mirroring
//! `deploy/backup-tool`'s own shape) and run as a one-shot container
//! joined to `compose.dev.yaml`'s own network, where `http://sms-gateway:8080`
//! is directly reachable — so the address this tool connects to and the
//! address the gateway is actually configured with are simply the *same*
//! string, and neither this tool nor the SDK needs to split anything.
//! `examples/rust/sms-send`'s own single-`--issuer` shape was never wrong
//! either, for the identical reason: a real integrator's `sms-gateway` is
//! one address full stop, which is exactly what running in-network gives
//! this tool too.
//!
//! # Why not `cargo xtask e2e-integration`
//!
//! `.xtask` deliberately depends on nothing that would slow its own
//! "stays fast" build promise down (`AGENTS.md`'s xtask section) — adding
//! `vsms-sdk-rust` (and, transitively, `cratestack-client`) to it would
//! break that promise for every other `cargo xtask` subcommand, not just
//! this one. `just e2e-integration` is the thin host-level orchestrator
//! instead (bring the stack up, provision a second client, run this
//! image on the same Compose network) — see the justfile's own recipe.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use cratestack::client_rust::ClientError;
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{PrivateKeyJwtConfig, SdkError, VsmsClient};

#[derive(Parser, Debug)]
#[command(
    about = "The joined integration story (#160): an integrator sends over real HTTP, the console's own credential reads it back, cross-principal, same App."
)]
struct Cli {
    /// The gateway's own address, reachable — and, once inside the
    /// Compose network this binary is meant to run in, *only* reachable —
    /// as `http://sms-gateway:8080`. Used as both the connection target
    /// and the client assertion's audience; see this file's own module
    /// doc for why those can't be split in practice.
    #[arg(long, env = "VSMS_E2E_GATEWAY_URL")]
    gateway_url: String,

    #[arg(long, env = "VSMS_E2E_INTEGRATOR_CLIENT_ID")]
    integrator_client_id: String,

    #[arg(long, env = "VSMS_E2E_INTEGRATOR_KEY_PATH")]
    integrator_key_path: std::path::PathBuf,

    #[arg(long, env = "VSMS_E2E_CONSOLE_CLIENT_ID")]
    console_client_id: String,

    #[arg(long, env = "VSMS_E2E_CONSOLE_KEY_PATH")]
    console_key_path: std::path::PathBuf,

    /// E.164 recipient — the default matches `scripts/e2e-integration.sh`'s
    /// own former default.
    #[arg(long, default_value = "+237677000222")]
    to: String,

    /// Must already be `approved` for this deployment's Orange account —
    /// `compose.dev.yaml`'s `seed-demo-app` seeds exactly `VSMS`
    /// (`sms-gateway seed-demo-app --sender-id`'s own default), unlike the
    /// old script's `VYMALO` (which came from a *different* fixture path,
    /// `send_test_message`, that this compose stack no longer runs).
    #[arg(long, default_value = "VSMS")]
    sender_id: String,

    #[arg(long)]
    client_ref: Option<String>,

    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let client_ref = cli.client_ref.unwrap_or_else(|| {
        format!(
            "e2e-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default()
        )
    });

    println!("1/4 building the integrator's own client (a second, independent AppClient)");
    let integrator = build_client(
        &cli.gateway_url,
        &cli.integrator_client_id,
        &cli.integrator_key_path,
        "sms:send sms:read",
    )?;

    println!("2/4 sending as the integrator, over real HTTP (clientRef={client_ref})");
    let args = SendMessageInput {
        to: cli.to,
        body: "Hello from the vsms e2e-integration scenario (#160)".to_owned(),
        senderId: Some(cli.sender_id),
        class: None,
        clientRef: Some(client_ref.clone()),
        scheduledAt: None,
        validityMinutes: None,
    };
    let outcome = integrator
        .send_message(args, None)
        .await
        .context("sendMessage as the integrator")?;
    let message_id = outcome.result.messageId.clone();
    println!(
        "    message id: {message_id} (initial state: {})",
        outcome.result.state
    );

    // Self-check, the same property `examples/rust/sms-send` already
    // proves for its own caller: the write actually landed, read back
    // through the same principal that made it, not just echoed by the
    // mutation's own response.
    let self_read = integrator
        .get_message(&message_id)
        .await
        .context("reading the message back as the integrator")?;
    let app_id = self_read.appId.clone();
    println!("    App: {app_id}");

    println!("3/4 building the console's own client (a second, independent principal)");
    let console = build_client(
        &cli.gateway_url,
        &cli.console_client_id,
        &cli.console_key_path,
        "sms:read sms:send",
    )?;

    println!(
        "4/4 polling GET /messages/{{id}} AS THE CONSOLE — the exact route \
         packages/gateway/src/messages.ts's getMessageById calls — until delivered"
    );
    let deadline = Instant::now() + Duration::from_secs(cli.timeout_secs);
    let mut last_state = String::new();
    let mut states_seen = Vec::new();

    loop {
        let result = console.get_message(&message_id).await;
        let message = match result {
            Ok(message) => message,
            Err(error) if is_not_found(&error) => {
                bail!(
                    "GET /messages/{message_id} returned 404 under the CONSOLE's own credential. \
                     Per packages/gateway/src/messages.ts's own module doc (point 9), sms-api \
                     cannot distinguish \"never existed\" from \"exists but belongs to another \
                     App\" — this means the console's principal (App {app_id}) cannot see a \
                     message that unquestionably exists (it was just sent and read back \
                     successfully under the integrator's own credential). THIS IS A FINDING, \
                     not a bug to route around: report it, do not retry past it."
                );
            }
            Err(error) => return Err(error).context("GET /messages/{id} as the console"),
        };

        if message.appId != app_id {
            bail!(
                "GET /messages/{message_id} returned appId={}, expected {app_id}",
                message.appId
            );
        }

        let state = message.state.to_string();
        if state != last_state {
            println!("    [{}] state={state}", now_hhmmss());
            states_seen.push(state.clone());
            last_state = state.clone();
        }

        if state == "delivered" {
            break;
        }
        if matches!(
            state.as_str(),
            "failed" | "rejected" | "expired" | "undelivered"
        ) {
            bail!("message {message_id} reached a terminal non-delivered state: {state}");
        }

        if Instant::now() >= deadline {
            bail!(
                "message {message_id} did not reach delivered within {}s (last state: \
                 {last_state})",
                cli.timeout_secs
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    println!();
    println!("PASSED");
    println!("    App:                 {app_id}");
    println!("    integrator client:   {}", cli.integrator_client_id);
    println!("    console client:      {}", cli.console_client_id);
    println!("    message id:          {message_id}");
    println!("    clientRef:           {client_ref}");
    println!("    state progression:   {}", states_seen.join(" -> "));
    println!();
    println!(
        "    (Orange is FAKED end to end here — sms-fake-orange, not a real carrier. See #36.)"
    );

    Ok(())
}

fn build_client(
    gateway_url: &str,
    client_id: &str,
    key_path: &std::path::Path,
    scope: &str,
) -> Result<VsmsClient> {
    let config = PrivateKeyJwtConfig::from_key_path(gateway_url, client_id, key_path, scope)
        .with_context(|| format!("loading the private key at {}", key_path.display()))?;
    VsmsClient::private_key_jwt(gateway_url, config).context("building the vsms client")
}

fn is_not_found(error: &SdkError) -> bool {
    matches!(
        error,
        SdkError::Client(ClientError::Remote { status, .. }) if status.as_u16() == 404
    )
}

fn now_hhmmss() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs_of_day = now % 86400;
    format!(
        "{:02}:{:02}:{:02}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}
