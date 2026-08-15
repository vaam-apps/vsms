#![doc = include_str!("main.md")]

use std::net::TcpListener;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use sms_fake_orange::{DlrStatus, DlrStep, FakeOrange, FaultPolicy, SubmitDecision, TokenPolicy};
use tracing::{info, warn};

/// How the fake picks a `SubmitDecision` per submit call.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FaultMode {
    /// Every submit is accepted, then reports `delivered` after
    /// `--dlr-delay-ms` — the default, and the one a demo should use.
    Happy,
    /// A reproducible, weighted mix of realistic successes and failures —
    /// the same distribution `backends/crates/sms-worker`'s chaos suite drives.
    /// `--seed` picks which draw; the same seed against the same call
    /// sequence always draws the same decisions.
    Seeded,
}

/// `sms-fake-orange` — a development/demo fake of Orange Cameroon's SMS API.
/// **Not for production use** — see this binary's own module doc.
#[derive(Debug, Parser)]
#[command(
    name = "sms-fake-orange",
    version,
    about = "Development/demo fake of Orange Cameroon's SMS API — NOT a real provider, NOT for production use"
)]
struct Cli {
    /// Address to bind the fake's HTTP server on. Point `ORANGE_CM_BASE_URL`
    /// at `http://<this>` for `sms-gateway`/`sms-worker`.
    #[arg(long, env = "FAKE_ORANGE_BIND_ADDR", default_value = "127.0.0.1:8090")]
    bind_addr: String,

    /// Full URL of the DLR webhook to POST delivery notifications back to —
    /// matches `backends/apps/sms-gateway`'s own `POST /dlr/{providerKey}` route, e.g.
    /// `http://127.0.0.1:8080/dlr/orange_cm`.
    #[arg(long, env = "FAKE_ORANGE_DLR_ENDPOINT")]
    dlr_endpoint: String,

    /// E.164 without the `tel:` scheme — must match whatever
    /// `ORANGE_CM_SENDER_NUMBER` `sms-worker`'s `dispatch` role (and
    /// `sms-gateway`) are configured with, since it's part of the submit
    /// URL path.
    #[arg(long, env = "FAKE_ORANGE_SENDER_NUMBER")]
    sender_number: String,

    /// How long after a submit is received to fire the `delivered` DLR, in
    /// the default `happy` fault mode. Ignored in `seeded` mode —
    /// `sms_fake_orange::fault`'s own weighted distribution picks its own
    /// delays.
    #[arg(long, env = "FAKE_ORANGE_DLR_DELAY_MS", default_value_t = 2000)]
    dlr_delay_ms: u64,

    /// See [`FaultMode`].
    #[arg(
        long,
        env = "FAKE_ORANGE_FAULT_MODE",
        value_enum,
        default_value_t = FaultMode::Happy
    )]
    fault_mode: FaultMode,

    /// Seed for `--fault-mode seeded`. Ignored in `happy` mode.
    #[arg(long, env = "FAKE_ORANGE_SEED", default_value_t = 1)]
    seed: u64,

    /// Answer every token request with `401` instead of a token — demos the
    /// "credential revoked mid-flight" failure mode
    /// (`ProviderError::Permanent`) instead of the happy path. Independent
    /// of `--fault-mode`: this governs the token endpoint, not the submit
    /// endpoint.
    #[arg(long, env = "FAKE_ORANGE_REJECT_TOKENS")]
    reject_tokens: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Variables already in the environment win; dotenvy never overwrites.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_fake_orange=info,sms_fake_orange_bin=info".into()),
        )
        .init();

    let cli = Cli::parse();

    let listener =
        TcpListener::bind(&cli.bind_addr).with_context(|| format!("binding {}", cli.bind_addr))?;
    let bound_addr = listener
        .local_addr()
        .context("reading the bound address back")?;

    let policy = match cli.fault_mode {
        FaultMode::Happy => {
            FaultPolicy::always(SubmitDecision::accepted_with_dlrs(vec![DlrStep::after(
                Duration::from_millis(cli.dlr_delay_ms),
                DlrStatus::Delivered,
            )]))
        }
        FaultMode::Seeded => FaultPolicy::seeded(cli.seed),
    };
    let token_policy = if cli.reject_tokens {
        TokenPolicy::AlwaysUnauthorized
    } else {
        TokenPolicy::Always
    };

    warn!(
        bind_addr = %bound_addr,
        dlr_endpoint = %cli.dlr_endpoint,
        sender_number = %cli.sender_number,
        fault_mode = ?cli.fault_mode,
        reject_tokens = cli.reject_tokens,
        "sms-fake-orange starting — THIS IS A DEVELOPMENT/DEMO IMPERSONATION OF ORANGE \
         CAMEROON'S SMS API. It sends no real SMS to any real handset. Never point a production \
         deployment at it."
    );

    let fake = FakeOrange::start_on(
        listener,
        policy,
        token_policy,
        cli.dlr_endpoint.clone(),
        &cli.sender_number,
    )
    .await;

    info!(base_url = %fake.base_url(), "sms-fake-orange ready — waiting for submit calls");

    shutdown_signal().await;
    info!(
        submits_received = fake.ledger().submits().len(),
        "sms-fake-orange shutting down"
    );
    Ok(())
}

/// Resolve on SIGINT *or* SIGTERM, matching `sms-gateway`/`sms-worker`'s own
/// shutdown handling.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("installing a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
