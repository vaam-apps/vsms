//! #160's joined integration story, ported off the old
//! `scripts/e2e-integration.sh`: an external integrator sends a message
//! over real HTTP, then the admin console's *own* credential — a second,
//! independent principal under the same `App` — reads that exact message
//! id back, until it reaches `delivered`. See that deleted script's own
//! git history for the bash-and-`openssl`-and-`curl`-and-`jq` version this
//! replaces.
//!
//! # What VS-3 changed
//!
//! Both principals now **sign**: the integrator and the console each hold an
//! Ed25519 key (a PKCS#8 PEM this tool minted with `keygen`), registered for
//! their own `AppClient` of the same `App` by `sms-gateway register-client
//! --client-name`, and every request is a COSE_Sign1 message verified against
//! the gateway's pinned `server-keys`. Nothing here acquires, caches or
//! refreshes a token; the old `--*-client-id` flags are gone because a signed
//! request names no client (the gateway finds it by the key).
//!
//! # Why this runs *inside* the compose network, not on the host
//!
//! The binary is built into its own small image (`ci/e2e-integration/
//! Dockerfile`, mirroring `deploy/backup-tool`'s own shape) and run as a
//! one-shot container joined to `compose.dev.yaml`'s own network, where
//! `http://sms-gateway:8080` is directly reachable. A sealed request is bound to
//! one audience (`vsms`) and one route, not to a host name, so a host process
//! could in principle reach the published port; running in-network keeps the
//! address this tool connects to the address the stack itself uses.
//!
//! # Why not `cargo xtask e2e-integration`
//!
//! `.xtask` deliberately depends on nothing that would slow its own
//! "stays fast" build promise down (`AGENTS.md`'s xtask section) - adding
//! `vsms-sdk-rust` (and, transitively, `cratestack-client`) to it would
//! break that promise for every other `cargo xtask` subcommand, not just
//! this one. `just e2e-integration` is the thin host-level orchestrator
//! instead (bring the stack up, mint and register two keys, run this image on
//! the same Compose network) - see the justfile's own recipe.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use cratestack::client_rust::ClientError;
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{SdkError, ServerKeys, SigningKey, VsmsClient};

#[derive(Parser, Debug)]
#[command(
    about = "The joined integration story (#160): an integrator sends over real HTTP, the console's own credential reads it back, cross-principal, same App."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Mints an Ed25519 key: writes the PKCS#8 PEM to `--out` (0600, refusing
    /// to overwrite) and prints the public half, unpadded base64url - the
    /// value `sms-gateway register-client --public-key` takes - on stdout.
    Keygen {
        #[arg(long)]
        out: std::path::PathBuf,
    },
    /// Runs the scenario.
    Run(Box<RunArgs>),
}

#[derive(Args, Debug)]
struct RunArgs {
    /// The gateway's own address, reachable - and, once inside the Compose
    /// network this binary is meant to run in - as `http://sms-gateway:8080`.
    #[arg(long, env = "VSMS_E2E_GATEWAY_URL")]
    gateway_url: String,

    /// The integrator's key (registered for its own `AppClient` of the App).
    #[arg(long, env = "VSMS_E2E_INTEGRATOR_KEY_PATH")]
    integrator_key_path: std::path::PathBuf,

    /// The console's key (registered for a second `AppClient` of the same
    /// App), the second, independent principal.
    #[arg(long, env = "VSMS_E2E_CONSOLE_KEY_PATH")]
    console_key_path: std::path::PathBuf,

    /// `sms-gateway server-keys`' output: the gateway's two public keys.
    #[arg(long, env = "VSMS_E2E_SERVER_KEYS_PATH")]
    server_keys_path: std::path::PathBuf,

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
    match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::Run(args) => run(*args).await,
    }
}

fn keygen(out: &std::path::Path) -> Result<()> {
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;

    let key = SigningKey::generate();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(out)
        .with_context(|| format!("creating {} (refusing to overwrite)", out.display()))?;
    file.write_all(key.to_pem().as_bytes())
        .with_context(|| format!("writing {}", out.display()))?;
    println!("{}", key.public_key_b64u());
    Ok(())
}

async fn run(cli: RunArgs) -> Result<()> {
    let server_keys = ServerKeys::parse_report(
        &std::fs::read_to_string(&cli.server_keys_path)
            .with_context(|| format!("reading {}", cli.server_keys_path.display()))?,
    )
    .context("the pinned server keys")?;

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
    let integrator = build_client(&cli.gateway_url, &cli.integrator_key_path, &server_keys)?;

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
    let console = build_client(&cli.gateway_url, &cli.console_key_path, &server_keys)?;

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
    println!(
        "    integrator key:      {}",
        cli.integrator_key_path.display()
    );
    println!(
        "    console key:         {}",
        cli.console_key_path.display()
    );
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
    key_path: &std::path::Path,
    server_keys: &ServerKeys,
) -> Result<VsmsClient> {
    let key = SigningKey::from_pem_file(key_path)
        .with_context(|| format!("loading the signing key at {}", key_path.display()))?;
    VsmsClient::signed(gateway_url, key, server_keys.clone()).context("building the vsms client")
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
