#![doc = include_str!("main.md")]

use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use sms_fake_aws::{
    DEFAULT_ACCESS_KEY_ID, DEFAULT_CONFIGURATION_SET, DEFAULT_REGION, DEFAULT_SECRET_ACCESS_KEY,
    DEFAULT_TOPIC_ARN, EventStep, FakeAws, FakeAwsConfig, FaultPolicy, Redelivery, SubmitDecision,
};
use tracing::{info, warn};

/// What every submit gets.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum FaultMode {
    /// Accept, report `TEXT_PENDING`, then `TEXT_DELIVERED` after
    /// `--delivery-delay-ms`. What a demo should use.
    Happy,
    /// Accept, then a final `TEXT_SUCCESSFUL` (carrier accepted, no
    /// handset receipt), as many real carriers report.
    CarrierOnly,
    /// Accept and never report: every message ends `expired`.
    Silent,
    /// The SMS sandbox: every submit refused with
    /// `DESTINATION_PHONE_NUMBER_NOT_VERIFIED`, so traffic fails over.
    Sandbox,
    /// Accept, then a final `TEXT_CARRIER_UNREACHABLE`.
    Unreachable,
}

/// `sms-fake-aws` — a development/demo fake of AWS End User Messaging SMS
/// and its SNS receipts. **Not for production use.**
#[derive(Debug, Parser)]
#[command(
    name = "sms-fake-aws",
    version,
    about = "Development/demo fake of AWS End User Messaging SMS and its SNS receipts — NOT AWS, NOT for production use"
)]
struct Cli {
    /// Address to bind. Point `AWS_SMS_ENDPOINT_URL` (sms-worker) and
    /// `AWS_SMS_FAKE_SNS_CERT_BASE` (sms-gateway, `fake-aws` build) at
    /// `http://<this>/`.
    #[arg(long, env = "FAKE_AWS_BIND_ADDR", default_value = "127.0.0.1:8095")]
    bind_addr: String,

    /// Full URL notifications are posted to, e.g.
    /// `http://sms-gateway:8080/dlr/aws_sms`.
    #[arg(long, env = "FAKE_AWS_DLR_ENDPOINT")]
    dlr_endpoint: String,

    /// Region: the Signature Version 4 scope the fake expects, and the topic's.
    #[arg(long, env = "FAKE_AWS_REGION", default_value = DEFAULT_REGION)]
    region: String,

    /// The one access key id accepted.
    #[arg(long, env = "FAKE_AWS_ACCESS_KEY_ID", default_value = DEFAULT_ACCESS_KEY_ID)]
    access_key_id: String,

    /// Its secret. A fixed, public demo value by default.
    #[arg(
        long,
        env = "FAKE_AWS_SECRET_ACCESS_KEY",
        default_value = DEFAULT_SECRET_ACCESS_KEY,
        hide_env_values = true
    )]
    secret_access_key: String,

    /// The one configuration set that exists.
    #[arg(long, env = "FAKE_AWS_CONFIGURATION_SET", default_value = DEFAULT_CONFIGURATION_SET)]
    configuration_set: String,

    /// The topic notifications claim to come from. Its region must match
    /// `--region`.
    #[arg(long, env = "FAKE_AWS_TOPIC_ARN", default_value = DEFAULT_TOPIC_ARN)]
    topic_arn: String,

    /// See [`FaultMode`].
    #[arg(long, env = "FAKE_AWS_FAULT_MODE", value_enum, default_value_t = FaultMode::Happy)]
    fault_mode: FaultMode,

    /// Time from accept to the final event.
    #[arg(long, env = "FAKE_AWS_DELIVERY_DELAY_MS", default_value_t = 2000)]
    delivery_delay_ms: u64,

    /// POST attempts per notification, including the first.
    #[arg(long, env = "FAKE_AWS_REDELIVERY_ATTEMPTS", default_value_t = 4)]
    redelivery_attempts: u32,

    /// Seconds between attempts. SNS's own default is 20.
    #[arg(long, env = "FAKE_AWS_REDELIVERY_INTERVAL_SECS", default_value_t = 20)]
    redelivery_interval_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Before anything builds a reqwest client (see sms-gateway's main.rs).
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_fake_aws=info,sms_fake_aws_bin=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let delay = Duration::from_millis(cli.delivery_delay_ms);
    let decision = match cli.fault_mode {
        FaultMode::Happy => SubmitDecision::delivered_after(delay),
        FaultMode::CarrierOnly => {
            SubmitDecision::accepted_with_events(vec![EventStep::carrier_accepted(delay)])
        }
        FaultMode::Silent => SubmitDecision::accepted_silently(),
        FaultMode::Sandbox => SubmitDecision::sandbox_unverified(),
        FaultMode::Unreachable => {
            SubmitDecision::accepted_with_events(vec![EventStep::unreachable(delay)])
        }
    };

    let listener = tokio::net::TcpListener::bind(&cli.bind_addr)
        .await
        .with_context(|| format!("binding {}", cli.bind_addr))?;
    let bound = listener.local_addr().context("reading the bound address")?;

    warn!(
        bind_addr = %bound,
        dlr_endpoint = %cli.dlr_endpoint,
        topic_arn = %cli.topic_arn,
        fault_mode = ?cli.fault_mode,
        "sms-fake-aws starting — THIS IS A DEVELOPMENT/DEMO IMPERSONATION OF AWS END USER \
         MESSAGING SMS. It sends no SMS to any handset. Never point a production deployment at it."
    );

    let fake = FakeAws::start_on(
        listener,
        FakeAwsConfig {
            region: cli.region,
            access_key_id: cli.access_key_id,
            secret_access_key: cli.secret_access_key,
            configuration_set: cli.configuration_set,
            topic_arn: cli.topic_arn,
            dlr_endpoint: cli.dlr_endpoint,
            redelivery: Redelivery {
                attempts: cli.redelivery_attempts,
                interval: Duration::from_secs(cli.redelivery_interval_secs),
            },
        },
        FaultPolicy::always(decision),
    );
    info!(
        base_url = fake.base_url(),
        signing_cert_url = fake.signing_cert_url(),
        "sms-fake-aws ready"
    );

    shutdown_signal().await;
    info!(
        submits_received = fake.ledger().submits().len(),
        "sms-fake-aws shutting down"
    );
    Ok(())
}

/// SIGINT or SIGTERM.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("installing a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
