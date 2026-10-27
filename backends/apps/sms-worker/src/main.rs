#![doc = include_str!("main.md")]

use anyhow::{Context, Result, bail};
use clap::Parser;
use sms_provider::SmsProvider;
use sms_worker::{Cardinality, Role, WorkerContext};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Where the heartbeat task below touches a file for the container's
/// `HEALTHCHECK` to read the mtime of. Not a CLI flag — this is
/// operational plumbing for the container (#139), not worker behavior an
/// operator would ever want to tune per deployment.
const DEFAULT_HEALTH_FILE: &str = "/tmp/sms-worker-healthy";

/// How often [`spawn_heartbeat`] touches the health file. Well under the
/// container `HEALTHCHECK`'s own staleness threshold (90s, see
/// `backends/apps/sms-worker/Dockerfile`) so a couple of missed ticks under load
/// don't false-positive a restart.
const HEALTH_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// How long a shutdown waits for singleton roles to release their leases
/// before giving up and letting the process exit anyway. Generous relative
/// to a single `pg_advisory_unlock` round trip — this is a backstop against
/// a wedged connection, not the expected duration. If it fires, the lease's
/// `Drop` and Postgres's own session semantics still release the lock (see
/// `sms_worker::lease`'s module doc) — just via the slower path this exists
/// to avoid.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// How old [`DEFAULT_HEALTH_FILE`]'s mtime may be before
/// [`healthcheck_command`] reports unhealthy. Matches the shell-based check
/// this replaced (`backends/apps/sms-worker/Dockerfile`'s old `HEALTHCHECK`,
/// `deploy/charts/vsms/values.yaml`'s worker exec probes) — well above
/// [`HEALTH_HEARTBEAT_INTERVAL`] so a couple of missed ticks under load
/// don't false-positive a restart.
const HEALTH_STALE_THRESHOLD: Duration = Duration::from_secs(90);

/// `sms-worker --roles dispatch,drain,scheduler,hooks,jobs` — see §9.2's
/// deployment diagram for why a real deployment runs two of these with
/// different `--roles` values.
#[derive(Debug, Parser)]
#[command(name = "sms-worker", version, about = "A2P SMS gateway worker")]
struct Cli {
    /// Comma-separated roles to run in this process. Each of §7.1's six
    /// names, at least one, no duplicates — running the same role twice in
    /// one process wastes a task without changing what actually runs.
    #[arg(long, env = "SMS_WORKER_ROLES", value_delimiter = ',')]
    roles: Vec<String>,

    /// Every role's queries run against this, not just singleton roles'
    /// leases — required unconditionally, the same way `sms-gateway`
    /// requires it.
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,

    /// #70/#71: `GET /metrics`, this process's own Prometheus text
    /// exposition — see this module's own doc for why this binary has one
    /// now, and `sms_api::metrics`'s module doc for why it's a second,
    /// separate listener rather than a route on anything else. Loopback by
    /// default for the same "never faces the internet" reasoning
    /// `sms-gateway`'s own `--metrics-listen` documents.
    #[arg(
        long,
        env = "SMS_WORKER_METRICS_LISTEN_ADDR",
        default_value = "127.0.0.1:9091"
    )]
    metrics_listen: String,

    /// Maximum pooled connections — mirrors `sms-gateway`'s own default.
    #[arg(long, env = "SMS_WORKER_DB_MAX_CONNECTIONS", default_value_t = 10)]
    db_max_connections: u32,

    /// Identifies this process to the claim loop (`leaseOwner`, and logged
    /// on a denied claim) — not a security boundary, just an operator-
    /// visible label. Defaults to `hostname:pid`, which is stable enough to
    /// recognise across a restart in logs without requiring every deploy to
    /// set one explicitly.
    #[arg(long, env = "SMS_WORKER_ID")]
    worker_id: Option<String>,

    /// PKCS#8 PEM Ed25519 private key that seals a `sealed` webhook endpoint's
    /// deliveries — the same file `sms-gateway serve` signs its answers with
    /// (`--envelope-signing-key-file`), so a receiver pins one pair of public
    /// keys for both (VS-8). Read only when `hooks` is one of `--roles`. Without
    /// it `hooks` still starts and still delivers every `hmac` endpoint (VS-8R:
    /// an app that has not moved its receivers onto the seal needs no key), but
    /// every `sealed` endpoint's attempts fail until it is mounted, and the
    /// worker says so at start. A path, not a secret: the file is the secret and
    /// must be mounted `0400`; it never goes in the database.
    #[arg(long, env = "SMS_ENVELOPE_SIGNING_KEY_FILE")]
    envelope_signing_key_file: Option<std::path::PathBuf>,

    /// `OAuth2` `client_credentials` client id for Orange Cameroon's SMS
    /// API — required only when `dispatch` is one of `--roles`, since it's
    /// the only role that submits through a provider today (#33).
    #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
    orange_client_id: Option<String>,

    /// Paired with `orange_client_id` — see its doc for when this is
    /// required. Never logged; `OrangeCmConfig` holds it only long enough
    /// to fetch and cache a bearer token (`sms-provider-orange-cm`'s own
    /// `token` module).
    #[arg(long, env = "ORANGE_CM_CLIENT_SECRET", hide_env_values = true)]
    orange_client_secret: Option<String>,

    /// E.164 without the `tel:` scheme — see `OrangeCmConfig::sender_number`
    /// for why the scheme is added by the adapter, not this binary.
    #[arg(long, env = "ORANGE_CM_SENDER_NUMBER")]
    orange_sender_number: Option<String>,

    /// Overridable so a real Orange sandbox (not just this crate's own
    /// `wiremock`-backed tests) can be pointed at without a code change.
    #[arg(
        long,
        env = "ORANGE_CM_BASE_URL",
        default_value = "https://api.orange.com"
    )]
    orange_base_url: String,

    /// The `OAuth2` `client_credentials` client id for MTN's direct API
    /// (MADAPI) — see `sms-provider-mtn`'s module doc for the real,
    /// vendored contract this replaces the previous aggregator-shaped
    /// static-Bearer-key auth with. Optional at the CLI level for the
    /// same reason every Orange flag above is: only `dispatch` ever
    /// reads the registry this feeds, so a deployment running only
    /// `hooks`/`jobs`/`drain`/`scheduler` needs neither provider's
    /// credentials at all. Paired with the other `--mtn-*` flags below —
    /// see [`mtn_provider`]'s own all-or-none check.
    #[arg(long, env = "MTN_CLIENT_ID")]
    mtn_client_id: Option<String>,

    /// Paired with `mtn_client_id`. Never logged.
    #[arg(long, env = "MTN_CLIENT_SECRET", hide_env_values = true)]
    mtn_client_secret: Option<String>,

    /// The approved short code MADAPI's own
    /// `outboundSMSMessageRequest.serviceCode` requires unconditionally
    /// on every submit — see `sms_provider_mtn::MtnConfig::service_code`'s
    /// own doc. Part of the all-or-none set: MADAPI's own Swagger marks
    /// this field mandatory.
    #[arg(long, env = "MTN_SERVICE_CODE")]
    mtn_service_code: Option<String>,

    /// The approved alphanumeric sender ID MTN submits under, used only
    /// as a fallback when a specific message's own resolved sender is
    /// empty — see `sms_provider_mtn::MtnProvider::sender_address`'s own
    /// doc. **Not** part of the all-or-none set below, unlike every
    /// other `--mtn-*` flag: MADAPI's own Swagger marks `senderAddress`
    /// optional, so requiring an operator to set this would enforce a
    /// constraint MADAPI itself doesn't have.
    #[arg(long, env = "MTN_SENDER_ID")]
    mtn_sender_id: Option<String>,

    /// MADAPI's API host — now a real, documented default
    /// (`https://api.mtn.com`, matching the vendored Swagger's own
    /// `host`), unlike the previous aggregator shape which had none to
    /// bake in (see `sms_provider_mtn::MtnConfig::base_url`'s own doc).
    #[arg(long, env = "MTN_BASE_URL", default_value = "https://api.mtn.com")]
    mtn_base_url: String,

    /// The submission rate this specific MADAPI contract allows, in
    /// messages per second — a negotiated commercial term with no public
    /// number to default to, unlike Orange's hard, published, self-service
    /// 5 TPS ceiling (see `sms_provider_mtn::MtnConfig::tps_ceiling`'s own
    /// doc). Required alongside the other `--mtn-*` flags rather than
    /// defaulted: this repo's own standing preference is no default that
    /// invents a fact (the same reasoning `HashPepper` has no `Default`,
    /// and why `MtnConfig` itself derives no `Default` impl at all), and
    /// an invented TPS ceiling would silently either throttle a real
    /// contract far below what it actually allows, or let `dispatch` submit
    /// faster than a real contract permits and get rate-limited or
    /// suspended.
    #[arg(long, env = "MTN_TPS_CEILING")]
    mtn_tps_ceiling: Option<f64>,

    /// What one segment costs on this contract, in XAF. `Decimal`, never a
    /// float — this is money (this repo's own standing rule; see
    /// `sms_provider_mtn::MtnConfig::cost_per_segment_xaf`'s own doc). Same
    /// required-together reasoning as `--mtn-tps-ceiling` above: a
    /// per-segment cost is a negotiated commercial term, not something
    /// safe to guess a default for — an invented cost would misprice
    /// every MTN message's `estimatedCostXaf`.
    #[arg(long, env = "MTN_COST_PER_SEGMENT_XAF")]
    mtn_cost_per_segment_xaf: Option<rust_decimal::Decimal>,

    /// Whether this specific MADAPI relationship has an alphanumeric
    /// sender registered and approved with MTN. Defaults to `false` —
    /// the same safer default
    /// `sms_provider_mtn::MtnConfig::supports_alphanumeric_sender`'s own
    /// doc argues for, because an unregistered alphanumeric sender
    /// submitted to MTN risks silent rewriting or dropping rather than a
    /// clean rejection this adapter could classify and route around.
    /// Unlike the other required `--mtn-*` flags, this one is not part of
    /// the all-or-none check: it has a safe, always-correct default
    /// whether or not MTN is configured at all, so nothing is lost by
    /// letting clap populate it unconditionally.
    #[arg(
        long,
        env = "MTN_SUPPORTS_ALPHANUMERIC_SENDER",
        default_value_t = false
    )]
    mtn_supports_alphanumeric_sender: bool,

    /// AWS region the End User Messaging SMS account sends from (e.g.
    /// `eu-west-1`). One of the six `--aws-sms-*` flags that must be set
    /// together or not at all — see [`aws_sms_provider`].
    #[arg(long, env = "AWS_SMS_REGION")]
    aws_sms_region: Option<String>,

    /// Access key id of an IAM user allowed `sms-voice:SendTextMessage`.
    /// Long-lived keys only — see `sms-provider-aws-sms`'s module doc.
    #[arg(long, env = "AWS_SMS_ACCESS_KEY_ID")]
    aws_sms_access_key_id: Option<String>,

    /// The secret half of that access key. Never logged.
    #[arg(long, env = "AWS_SMS_SECRET_ACCESS_KEY", hide_env_values = true)]
    aws_sms_secret_access_key: Option<String>,

    /// The configuration set whose SNS event destination delivers receipts
    /// to the gateway's `/dlr/aws_sms`. Required with the other
    /// `--aws-sms-*` flags: without one AWS reports nothing, and every
    /// message would end `expired`.
    #[arg(long, env = "AWS_SMS_CONFIGURATION_SET")]
    aws_sms_configuration_set: Option<String>,

    /// Message parts per second the account may send — an account quota,
    /// so never defaulted. Required with the other `--aws-sms-*` flags.
    #[arg(long, env = "AWS_SMS_TPS_CEILING")]
    aws_sms_tps_ceiling: Option<f64>,

    /// What one segment costs, converted to XAF (AWS prices in USD per
    /// destination country). Required with the other `--aws-sms-*` flags.
    #[arg(long, env = "AWS_SMS_COST_PER_SEGMENT_XAF")]
    aws_sms_cost_per_segment_xaf: Option<rust_decimal::Decimal>,

    /// A fixed origination identity for every message: a sender ID, phone
    /// number, pool, or one of their ARNs. When unset, each message's own
    /// approved sender ID is passed, and has to exist in the AWS account.
    #[arg(long, env = "AWS_SMS_ORIGINATION_IDENTITY")]
    aws_sms_origination_identity: Option<String>,

    /// `transactional` (OTPs, notifications) or `promotional`.
    #[arg(long, env = "AWS_SMS_MESSAGE_TYPE", default_value = "transactional")]
    aws_sms_message_type: sms_provider_aws_sms::MessageType,

    /// Whether the account sends under alphanumeric sender IDs to this
    /// deployment's destinations. Informational, like MTN's.
    #[arg(
        long,
        env = "AWS_SMS_SUPPORTS_ALPHANUMERIC_SENDER",
        default_value_t = false
    )]
    aws_sms_supports_alphanumeric_sender: bool,

    /// Endpoint override (a VPC endpoint, or a test double). Unset is
    /// `https://sms-voice.<region>.amazonaws.com/`.
    #[arg(long, env = "AWS_SMS_ENDPOINT_URL")]
    aws_sms_endpoint_url: Option<String>,

    /// Seconds AWS keeps trying to deliver one message (5 to 259200).
    /// Unset, AWS tries for up to 72 hours, so an OTP vsms already expired
    /// can still arrive days later; match the shortest validity of the
    /// traffic routed to AWS (15 minutes, `900`, for OTPs). Optional, and
    /// not part of the all-or-none group.
    #[arg(long, env = "AWS_SMS_TIME_TO_LIVE_SECONDS")]
    aws_sms_time_to_live_seconds: Option<u64>,
}

/// The one Orange provider `dispatch` submits through. `None` when
/// `dispatch` isn't among `--roles` — nothing constructs it, and nothing
/// needs to.
fn orange_provider(cli: &Cli) -> Result<Option<Arc<dyn SmsProvider>>> {
    match (
        &cli.orange_client_id,
        &cli.orange_client_secret,
        &cli.orange_sender_number,
    ) {
        (Some(client_id), Some(client_secret), Some(sender_number)) => {
            let mut config = sms_provider_orange_cm::OrangeCmConfig::production(
                client_id.clone(),
                client_secret.clone(),
                sender_number.clone(),
            );
            config.base_url.clone_from(&cli.orange_base_url);
            Ok(Some(Arc::new(
                sms_provider_orange_cm::OrangeCmProvider::new(config),
            )))
        }
        (None, None, None) => Ok(None),
        _ => bail!(
            "--orange-client-id, --orange-client-secret and --orange-sender-number must all be \
             set together, or none of them"
        ),
    }
}

/// The MTN provider `dispatch` submits through, now via MADAPI directly
/// (#61's crate, wired into a binary for the first time then; rewired
/// off the aggregator shape in this hard cutover). `None` when
/// `dispatch` isn't among `--roles`, or when none of the five required
/// `--mtn-*` flags (`client_id`/`client_secret`/`service_code`/
/// `tps_ceiling`/`cost_per_segment_xaf`) are set at all — mirrors
/// [`orange_provider`]'s own all-or-none shape, extended to five fields
/// instead of three because `MtnConfig` carries two more required,
/// `Default`-less commercial terms Orange's own config doesn't
/// (`tps_ceiling`, `cost_per_segment_xaf` — see that struct's own doc
/// for why neither has an invented default). `mtn_sender_id` and
/// `mtn_supports_alphanumeric_sender` are deliberately excluded from
/// this tuple: `sender_id` has a safe, always-usable `None` (MADAPI's
/// own Swagger marks `senderAddress` optional), and
/// `supports_alphanumeric_sender` has a safe default (`false`)
/// regardless of whether MTN is configured — both are read directly off
/// `cli` inside the `Some` arm rather than gating presence.
fn mtn_provider(cli: &Cli) -> Result<Option<Arc<dyn SmsProvider>>> {
    match (
        &cli.mtn_client_id,
        &cli.mtn_client_secret,
        &cli.mtn_service_code,
        cli.mtn_tps_ceiling,
        cli.mtn_cost_per_segment_xaf,
    ) {
        (
            Some(client_id),
            Some(client_secret),
            Some(service_code),
            Some(tps_ceiling),
            Some(cost),
        ) => {
            let config = sms_provider_mtn::MtnConfig {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                service_code: service_code.clone(),
                sender_id: cli.mtn_sender_id.clone(),
                base_url: cli.mtn_base_url.clone(),
                tps_ceiling,
                cost_per_segment_xaf: cost,
                supports_alphanumeric_sender: cli.mtn_supports_alphanumeric_sender,
                // Same values `sms-provider-orange-cm::OrangeCmConfig::production`
                // bakes in — no `MtnConfig` equivalent exists to default
                // these from, and nothing about either timeout is
                // provider-specific.
                connect_timeout: std::time::Duration::from_secs(10),
                request_timeout: std::time::Duration::from_secs(30),
            };
            Ok(Some(Arc::new(sms_provider_mtn::MtnProvider::new(config))))
        }
        (None, None, None, None, None) => Ok(None),
        _ => bail!(
            "--mtn-client-id, --mtn-client-secret, --mtn-service-code, --mtn-tps-ceiling and \
             --mtn-cost-per-segment-xaf must all be set together, or none of them"
        ),
    }
}

/// The AWS End User Messaging SMS provider, `"aws_sms"`. Same all-or-none
/// shape as [`mtn_provider`], over the six values that have no safe
/// default: region, both halves of the access key, the configuration set
/// that carries receipts, and the two commercial terms. The origination
/// identity, message type, alphanumeric flag, endpoint and time to live
/// each have one.
fn aws_sms_provider(cli: &Cli) -> Result<Option<Arc<dyn SmsProvider>>> {
    match (
        &cli.aws_sms_region,
        &cli.aws_sms_access_key_id,
        &cli.aws_sms_secret_access_key,
        &cli.aws_sms_configuration_set,
        cli.aws_sms_tps_ceiling,
        cli.aws_sms_cost_per_segment_xaf,
    ) {
        (
            Some(region),
            Some(access_key_id),
            Some(secret_access_key),
            Some(configuration_set),
            Some(tps_ceiling),
            Some(cost),
        ) => {
            let provider =
                sms_provider_aws_sms::AwsSmsProvider::sender(sms_provider_aws_sms::AwsSmsConfig {
                    region: region.clone(),
                    access_key_id: access_key_id.clone(),
                    secret_access_key: secret_access_key.clone(),
                    configuration_set: configuration_set.clone(),
                    origination_identity: cli.aws_sms_origination_identity.clone(),
                    message_type: cli.aws_sms_message_type,
                    tps_ceiling,
                    cost_per_segment_xaf: cost,
                    supports_alphanumeric_sender: cli.aws_sms_supports_alphanumeric_sender,
                    endpoint_url: cli.aws_sms_endpoint_url.clone(),
                    time_to_live: cli
                        .aws_sms_time_to_live_seconds
                        .map(std::time::Duration::from_secs),
                    connect_timeout: std::time::Duration::from_secs(10),
                    request_timeout: std::time::Duration::from_secs(30),
                })?;
            Ok(Some(Arc::new(provider)))
        }
        (None, None, None, None, None, None) => Ok(None),
        _ => bail!(
            "--aws-sms-region, --aws-sms-access-key-id, --aws-sms-secret-access-key, \
             --aws-sms-configuration-set, --aws-sms-tps-ceiling and \
             --aws-sms-cost-per-segment-xaf must all be set together, or none of them"
        ),
    }
}

/// The sealer `hooks` delivers `sealed` endpoints with, built from
/// `--envelope-signing-key-file`.
///
/// `None` (and a warning) when `hooks` runs without the flag: the role starts,
/// delivers every `hmac` endpoint, and fails a `sealed` endpoint's attempt with a
/// message that names the flag, rather than a process that refuses to start for a
/// key an HMAC-only deployment never uses (VS-8R). A key that is given but cannot
/// be read is another matter and always an error: a path that was meant to work.
/// A process that does not run `hooks` ignores the flag entirely, so a
/// `dispatch`-only node needs no key and holds none.
///
/// # Errors
///
/// A file that cannot be read, or one that is not a PKCS#8 Ed25519 private key.
/// The text names the flag and the path, never the content.
fn webhook_sealer(cli: &Cli, roles: &[Role]) -> Result<Option<sms_webhook::Sealer>> {
    if !roles.contains(&Role::Hooks) {
        return Ok(None);
    }
    let Some(path) = cli.envelope_signing_key_file.as_ref() else {
        warn!(
            "hooks runs without --envelope-signing-key-file (SMS_ENVELOPE_SIGNING_KEY_FILE): \
             `hmac` webhook endpoints are delivered, every `sealed` endpoint's attempts fail \
             until the gateway's PKCS#8 PEM Ed25519 signing key is mounted"
        );
        return Ok(None);
    };
    let pem = std::fs::read(path)
        .with_context(|| format!("reading --envelope-signing-key-file {}", path.display()))?;
    let signer = sms_api::envelope::signer_from_pem(&pem)
        .with_context(|| format!("the envelope signing key {}", path.display()))?;
    let sealer = sms_webhook::Sealer::new(signer).context("building the webhook sealer")?;
    Ok(Some(sealer))
}

/// Build the [`sms_worker::ProviderRegistry`] this process holds
/// credentials for, keyed by each adapter's own [`SmsProvider::key`] — #62:
/// `dispatch::resolve_provider` looks a routed message's provider up by
/// exactly this string, matching `Provider.key` in the database. Empty
/// when `dispatch` isn't among `--roles` (nothing in any other role ever
/// reads this registry) — the startup check right after this call is what
/// makes "dispatch requires real credentials" an actual guarantee, not
/// just a hope. Two entries as of the MTN wiring (`"orange_cm"`,
/// `"mtn_cm"`) — either, both, or neither may be configured; a third
/// real adapter is a third `.insert(...)` here, not a redesign of this
/// function's shape.
fn build_provider_registry(cli: &Cli) -> Result<HashMap<String, Arc<dyn SmsProvider>>> {
    let mut providers: HashMap<String, Arc<dyn SmsProvider>> = HashMap::new();
    if let Some(orange) = orange_provider(cli)? {
        providers.insert(orange.key().to_owned(), orange);
    }
    if let Some(mtn) = mtn_provider(cli)? {
        providers.insert(mtn.key().to_owned(), mtn);
    }
    if let Some(aws) = aws_sms_provider(cli)? {
        providers.insert(aws.key().to_owned(), aws);
    }
    Ok(providers)
}

/// `hostname:pid` — stable enough to recognise a given process across a
/// restart in logs (the container id, under §9.2's Docker deployment)
/// without requiring every deploy to set `--worker-id` explicitly.
fn default_worker_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_owned());
    format!("{host}:{}", std::process::id())
}

/// This binary had no HTTP surface at all until #70/#71 gave it a
/// `/metrics` listener (this module's own doc) — and that listener still
/// answers a different question than a container `HEALTHCHECK` needs (#139:
/// is this process alive and unstuck, not what its counters currently
/// read), so there remains nothing a `curl`-style liveness check could
/// productively hit. A heartbeat file is that substitute: spawned
/// unconditionally, independent of which `--roles` this process runs, so it
/// says something a bare `pgrep sms-worker` in the `HEALTHCHECK` command
/// could not — a hung tokio runtime (a role deadlocked rather than merely
/// idling on `run`'s own `std::future::pending` stub, or a claim loop
/// wedged on a connection) stops touching this file even though the
/// process is still very much alive as far as `pgrep` (or a still-answering
/// `/metrics`, for that matter — this task loop and the metrics listener
/// are two independent tokio tasks) is concerned.
fn spawn_heartbeat(path: std::path::PathBuf, shutdown: CancellationToken) {
    tokio::spawn(async move {
        // `tokio::time::interval`'s first `tick()` resolves immediately, so
        // the file exists before the container's `HEALTHCHECK
        // --start-period` elapses rather than only after the first full
        // interval.
        let mut ticker = tokio::time::interval(HEALTH_HEARTBEAT_INTERVAL);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // A brief, infrequent local file write — not worth a
                    // tokio::fs dependency just to keep it off this task's
                    // own thread.
                    if let Err(error) = std::fs::write(&path, std::process::id().to_string()) {
                        warn!(%error, path = %path.display(), "failed to write worker heartbeat file");
                    }
                }
                () = shutdown.cancelled() => break,
            }
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    // Must run before anything constructs an HTTP client — see
    // `install_default_crypto_provider`'s own doc (backends/apps/sms-gateway/src/
    // main.rs carries the identical function, and the identical reasoning)
    // for why.
    install_default_crypto_provider();

    // Deliberately checked before `Cli::parse()`, not folded into `Cli` as
    // a `#[command(subcommand)]`: `Cli` is a flat, no-subcommand struct
    // today, and every existing invocation of this binary — the design
    // doc's own worked examples, `deploy/docker-compose.yml`, the Helm
    // chart — runs it as plain `sms-worker --roles ... `, with no
    // subcommand at all. Restructuring `Cli` around a subcommand just to
    // add this one exec-form health check would change that invocation
    // shape for everyone. Checked ahead of `Cli::parse()` because `Cli`
    // has no positional arguments, so `sms-worker healthcheck` would
    // otherwise fail clap's own "unexpected argument" parsing before this
    // branch ever got a chance to run.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck_command();
    }

    // Variables already in the environment win; dotenvy never overwrites.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_worker=info,sms_worker_bin=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let roles = parse_roles(&cli.roles)?;

    let providers = build_provider_registry(&cli)?;
    if roles.contains(&Role::Dispatch) && providers.is_empty() {
        bail!(
            "--roles includes dispatch, which needs at least one provider's credentials to \
             submit anything: either --orange-client-id, --orange-client-secret and \
             --orange-sender-number, or --mtn-client-id, --mtn-client-secret, \
             --mtn-service-code, --mtn-tps-ceiling and --mtn-cost-per-segment-xaf, or the six \
             --aws-sms-* flags (or their env vars)"
        );
    }

    let webhook_sealer = webhook_sealer(&cli, &roles)?;

    let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
        .max_connections(cli.db_max_connections)
        .connect(&cli.database_url)
        .await
        .context("connecting to Postgres")?;
    let ctx = WorkerContext {
        db: sms_api::schema::Cratestack::builder(pool).build(),
        providers: Arc::new(providers),
        webhook_sealer,
    };

    // Unconditional — not gated on `drain` being one of `--roles`. Every
    // role in this process shares this one `Cratestack` (cloned per task,
    // sharing the same underlying `CratestackEventBus` via its `Arc`-backed
    // internals), and `dispatch`/`jobs::expire_stale` write to `Message`
    // themselves — a write on an emitting model with no subscriber
    // registered on this process's own runtime doesn't wait for `drain` to
    // pick it up later, it gets marked delivered with nothing done, by the
    // library's own automatic post-commit drain, and is lost. See
    // `sms_api::webhooks`'s own module doc for the full mechanism.
    sms_api::webhooks::register_subscribers(&ctx.db);

    let worker_id = cli.worker_id.clone().unwrap_or_else(default_worker_id);

    info!(
        roles = %roles.iter().copied().map(Role::as_str).collect::<Vec<_>>().join(","),
        worker_id,
        "sms-worker starting"
    );

    let shutdown = CancellationToken::new();

    let health_file = std::env::var("SMS_WORKER_HEALTH_FILE")
        .unwrap_or_else(|_| DEFAULT_HEALTH_FILE.to_owned())
        .into();
    spawn_heartbeat(health_file, shutdown.clone());

    let mut tasks = tokio::task::JoinSet::new();
    // #70/#71: bound before any role task starts, so a bind failure (an
    // already-used port, an invalid address) aborts startup the same way
    // every other startup precondition in this binary does, rather than
    // surfacing only once the first metrics scrape fails. Spawned into
    // `tasks` itself — see `spawn_metrics_task`'s own doc for why.
    spawn_metrics_task(&cli.metrics_listen, shutdown.clone(), &mut tasks).await?;

    for role in roles {
        match role.cardinality() {
            Cardinality::Singleton => {
                tasks.spawn(sms_worker::run_singleton(
                    role,
                    cli.database_url.clone(),
                    ctx.clone(),
                    worker_id.clone(),
                    shutdown.clone(),
                ));
            }
            // No lease to hold, so nothing to release — but still routed
            // through the same cancellation token as singleton roles, so
            // every task in `tasks` is guaranteed to return once shutdown is
            // requested and the drain loop below can't hang waiting on one
            // that never observes it.
            Cardinality::ScaleToN => {
                let cancel = shutdown.clone();
                let ctx = ctx.clone();
                let worker_id = worker_id.clone();
                tasks.spawn(async move {
                    tokio::select! {
                        () = sms_worker::run(role, ctx, &worker_id) => {}
                        () = cancel.cancelled() => {}
                    }
                });
            }
        }
    }

    tokio::select! {
        () = shutdown_signal() => {
            info!("shutdown signal received; releasing leases");
            shutdown.cancel();
        }
        // A role task returning at all — success or panic — is the one
        // thing that should never happen before shutdown is requested:
        // sms_worker::run idles forever, and run_singleton only returns
        // once `shutdown` is cancelled. JoinSet surfaces a panic as Err
        // here rather than letting it vanish silently.
        Some(finished) = tasks.join_next() => {
            finished.context("a role task panicked")?;
            bail!("a role task returned before shutdown was requested");
        }
    }

    if tokio::time::timeout(SHUTDOWN_GRACE, drain(&mut tasks))
        .await
        .is_err()
    {
        warn!(
            grace_secs = SHUTDOWN_GRACE.as_secs(),
            "role tasks did not finish releasing leases within the shutdown grace period; \
             exiting anyway — the connection closing still releases any held lock"
        );
    }

    Ok(())
}

/// Binds `listen` and spawns `sms_api::metrics::router()` on it, into
/// `tasks` — not a bare `tokio::spawn` — since this listener is expected to
/// run for the whole process lifetime exactly like every role task, so it
/// exiting early (a panic, the listener itself erroring) is just as much a
/// bug worth `main`'s own `Some(finished) = tasks.join_next()` arm noticing,
/// not a silently-abandoned background task. Pulled out of `main` purely to
/// stay under clippy's `too_many_lines` limit, same convention
/// `backends/apps/sms-gateway/src/main.rs::spawn_metrics_server` documents.
async fn spawn_metrics_task(
    listen: &str,
    shutdown: CancellationToken,
    tasks: &mut tokio::task::JoinSet<()>,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding metrics listener {listen}"))?;
    info!(listen, "sms-worker metrics listening");
    tasks.spawn(async move {
        if let Err(error) = axum::serve(listener, sms_api::metrics::router().into_make_service())
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        {
            warn!(%error, "metrics listener exited with an error");
        }
    });
    Ok(())
}

/// Wait for every remaining task to finish, logging (not propagating) any
/// panic — a panic during shutdown shouldn't turn a graceful exit into a
/// failing one, but it also shouldn't vanish.
async fn drain(tasks: &mut tokio::task::JoinSet<()>) {
    while let Some(finished) = tasks.join_next().await {
        if let Err(error) = finished {
            warn!(%error, "a role task panicked while shutting down");
        }
    }
}

/// Parse and validate `--roles` before anything is spawned, so a typo or a
/// duplicate fails at startup with one clear message instead of surfacing
/// as "why is `dispatch` missing" three services later.
fn parse_roles(raw: &[String]) -> Result<Vec<Role>> {
    if raw.is_empty() {
        bail!(
            "--roles must name at least one role (dispatch, drain, scheduler, hooks, jobs, smpp)"
        );
    }

    let mut seen = HashSet::new();
    let mut roles = Vec::with_capacity(raw.len());
    for name in raw {
        let role: Role = name
            .trim()
            .parse()
            .with_context(|| format!("invalid --roles value {name:?}"))?;
        if !seen.insert(role) {
            bail!("role {role} was named more than once in --roles");
        }
        roles.push(role);
    }
    Ok(roles)
}

/// Installs `ring` as the process-wide default `rustls` `CryptoProvider` —
/// see `backends/apps/sms-gateway/src/main.rs`'s identical function for the full
/// reasoning (why `ring` and not `aws-lc-rs`, and why the call stays now
/// that the `authkestra` stack that needed it is gone). `.ok()`, not
/// `.expect(...)`, for the same reason as that copy: the only failure mode
/// is "already installed," never a reason to abort startup.
fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The `sms-worker healthcheck` exec-form check — see `main`'s own comment
/// for why this is intercepted ahead of `Cli::parse()` rather than a real
/// clap subcommand, and `backends/apps/sms-worker/Dockerfile`'s header for why an
/// exec-form check exists at all now (a distroless `static` runtime image
/// has no `/bin/sh`, so the old `sh -c 'test -f ... && [ ... -lt 90 ]'`
/// `HEALTHCHECK`/exec-probe command can no longer run). Reproduces that
/// script's exact check in `std`-only Rust: [`DEFAULT_HEALTH_FILE`] (or
/// `SMS_WORKER_HEALTH_FILE`, same override the heartbeat task itself
/// honours) must exist and have been touched within
/// [`HEALTH_STALE_THRESHOLD`].
fn healthcheck_command() -> Result<()> {
    let path =
        std::env::var("SMS_WORKER_HEALTH_FILE").unwrap_or_else(|_| DEFAULT_HEALTH_FILE.to_owned());
    let modified = std::fs::metadata(&path)
        .with_context(|| format!("reading health file {path}"))?
        .modified()
        .with_context(|| format!("reading mtime of health file {path}"))?;
    let age = modified
        .elapsed()
        .with_context(|| format!("health file {path} has a mtime in the future"))?;
    if age < HEALTH_STALE_THRESHOLD {
        Ok(())
    } else {
        bail!(
            "unhealthy: health file {path} was last touched {age:?} ago, over the \
             {HEALTH_STALE_THRESHOLD:?} threshold"
        )
    }
}

/// Resolve on SIGINT *or* SIGTERM so in-flight work finishes.
///
/// `ctrl_c()` alone only catches SIGINT. §9.2 deploys this as a Docker
/// container, and `docker stop` / `kubectl rollout restart` send SIGTERM
/// first, SIGKILL only after the grace period elapses — SIGINT is never
/// sent in that path at all. Missing SIGTERM here would mean this branch
/// never fires under the deployment §9.2 actually describes, and the
/// process would always hit the force-kill timeout instead: silently, since
/// a container restarting slightly late looks identical to one restarting
/// correctly — and now that `run_singleton` releases its lease from this
/// branch (#28), missing SIGTERM would mean every restart falls back to the
/// slower drop-triggered release path instead of the fast explicit one, on
/// every single deploy, not just a hard kill.
///
/// Unix-only because `tokio::signal::unix` is: §9.2's deployment is Docker
/// Compose on a single VM, never Windows, so a `cfg(unix)` split with a
/// SIGINT-only fallback elsewhere costs nothing this binary needs.
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

#[cfg(test)]
mod tests {
    use super::{Cli, build_provider_registry, parse_roles, webhook_sealer};
    use sms_worker::Role;

    /// Every field explicitly, not `Default` (there is none) — a minimal,
    /// no-adapter-configured `Cli` a test can override individual fields
    /// on via struct-update syntax.
    fn base_cli() -> Cli {
        Cli {
            roles: Vec::new(),
            database_url: String::new(),
            metrics_listen: String::new(),
            db_max_connections: 10,
            worker_id: None,
            envelope_signing_key_file: None,
            orange_client_id: None,
            orange_client_secret: None,
            orange_sender_number: None,
            orange_base_url: "https://api.orange.com".to_owned(),
            mtn_client_id: None,
            mtn_client_secret: None,
            mtn_service_code: None,
            mtn_sender_id: None,
            mtn_base_url: "https://api.mtn.com".to_owned(),
            mtn_tps_ceiling: None,
            mtn_cost_per_segment_xaf: None,
            mtn_supports_alphanumeric_sender: false,
            aws_sms_region: None,
            aws_sms_access_key_id: None,
            aws_sms_secret_access_key: None,
            aws_sms_configuration_set: None,
            aws_sms_tps_ceiling: None,
            aws_sms_cost_per_segment_xaf: None,
            aws_sms_origination_identity: None,
            aws_sms_message_type: sms_provider_aws_sms::MessageType::Transactional,
            aws_sms_supports_alphanumeric_sender: false,
            aws_sms_endpoint_url: None,
            aws_sms_time_to_live_seconds: None,
        }
    }

    fn aws_cli() -> Cli {
        Cli {
            aws_sms_region: Some("eu-west-1".to_owned()),
            aws_sms_access_key_id: Some("AKIDEXAMPLE".to_owned()),
            aws_sms_secret_access_key: Some("secret".to_owned()),
            aws_sms_configuration_set: Some("vsms-events".to_owned()),
            aws_sms_tps_ceiling: Some(20.0),
            aws_sms_cost_per_segment_xaf: Some(rust_decimal::Decimal::new(30, 0)),
            ..base_cli()
        }
    }

    fn key_file(tag: &str, contents: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sms-worker-key-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("current.pem");
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// VS-8R: an HMAC-only deployment needs no signing key. `hooks` starts
    /// without one and holds no sealer, so its `hmac` endpoints are delivered
    /// and a `sealed` endpoint's attempt fails with a message naming the flag.
    #[test]
    fn hooks_without_the_signing_key_starts_and_holds_no_sealer() {
        let sealer = webhook_sealer(&base_cli(), &[Role::Hooks, Role::Jobs])
            .expect("hooks starts without a signing key");
        assert!(sealer.is_none());
    }

    /// A node that does not run `hooks` needs no key and loads none, even when
    /// a path is configured: the private key is held only where it is used.
    #[test]
    fn a_node_without_hooks_loads_no_key() {
        let cli = Cli {
            envelope_signing_key_file: Some(std::path::PathBuf::from("/does/not/exist.pem")),
            ..base_cli()
        };
        assert!(webhook_sealer(&cli, &[Role::Dispatch]).unwrap().is_none());
    }

    #[test]
    fn hooks_with_a_valid_key_builds_a_sealer() {
        let path = key_file("ok", &sms_api::envelope::pem_of_seed(&[5; 32]));
        let cli = Cli {
            envelope_signing_key_file: Some(path),
            ..base_cli()
        };
        assert!(webhook_sealer(&cli, &[Role::Hooks]).unwrap().is_some());
    }

    #[test]
    fn hooks_with_an_unreadable_or_foreign_key_names_the_path_and_not_the_content() {
        let missing = Cli {
            envelope_signing_key_file: Some(std::path::PathBuf::from("/does/not/exist.pem")),
            ..base_cli()
        };
        let text = format!(
            "{:#}",
            webhook_sealer(&missing, &[Role::Hooks]).unwrap_err()
        );
        assert!(text.contains("/does/not/exist.pem"), "{text}");

        let junk = Cli {
            envelope_signing_key_file: Some(key_file("junk", b"super-secret-looking-junk")),
            ..base_cli()
        };
        let text = format!("{:#}", webhook_sealer(&junk, &[Role::Hooks]).unwrap_err());
        assert!(text.contains("current.pem"), "{text}");
        assert!(!text.contains("super-secret-looking-junk"), "{text}");
    }

    /// The go-live shape: AWS alone, no Orange or MTN contract yet.
    #[test]
    fn aws_sms_alone_registers_under_its_own_key() {
        let providers = build_provider_registry(&aws_cli()).unwrap();
        assert_eq!(providers.len(), 1);
        assert!(providers.contains_key("aws_sms"));
    }

    /// The later shape the routing weights split between: every adapter in
    /// one process, each under its own key.
    #[test]
    fn aws_sms_registers_alongside_orange_and_mtn() {
        let cli = Cli {
            orange_client_id: Some("id".to_owned()),
            orange_client_secret: Some("secret".to_owned()),
            orange_sender_number: Some("+237677000000".to_owned()),
            mtn_client_id: Some("client-id".to_owned()),
            mtn_client_secret: Some("client-secret".to_owned()),
            mtn_service_code: Some("131".to_owned()),
            mtn_tps_ceiling: Some(20.0),
            mtn_cost_per_segment_xaf: Some(rust_decimal::Decimal::new(15, 0)),
            ..aws_cli()
        };
        let providers = build_provider_registry(&cli).unwrap();
        let mut keys: Vec<_> = providers.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["aws_sms", "mtn_cm", "orange_cm"]);
    }

    #[test]
    fn a_partial_aws_sms_configuration_is_an_error() {
        let cli = Cli {
            aws_sms_configuration_set: None,
            ..aws_cli()
        };
        let error = build_provider_registry(&cli).map(|_| ()).unwrap_err();
        assert!(
            error.to_string().contains("--aws-sms-configuration-set"),
            "{error}"
        );
    }

    #[test]
    fn an_aws_sms_configuration_that_could_never_send_is_an_error() {
        let cli = Cli {
            aws_sms_region: Some("Not A Region".to_owned()),
            ..aws_cli()
        };
        assert!(build_provider_registry(&cli).is_err());
    }

    #[test]
    fn the_aws_sms_time_to_live_is_optional_and_range_checked() {
        let with = |seconds| Cli {
            aws_sms_time_to_live_seconds: Some(seconds),
            ..aws_cli()
        };
        assert!(build_provider_registry(&with(900)).is_ok());
        assert!(build_provider_registry(&with(4)).is_err());
        assert!(build_provider_registry(&with(259_201)).is_err());
    }

    #[test]
    fn no_credentials_configured_is_an_empty_registry_not_an_error() {
        let providers = build_provider_registry(&base_cli()).unwrap();
        assert_eq!(providers.len(), 0);
    }

    #[test]
    fn orange_alone_registers_under_its_own_key() {
        let cli = Cli {
            orange_client_id: Some("id".to_owned()),
            orange_client_secret: Some("secret".to_owned()),
            orange_sender_number: Some("+237677000000".to_owned()),
            ..base_cli()
        };
        let providers = build_provider_registry(&cli).unwrap();
        assert_eq!(providers.len(), 1);
        assert!(providers.contains_key("orange_cm"));
    }

    #[test]
    fn mtn_alone_registers_under_its_own_key() {
        let cli = Cli {
            mtn_client_id: Some("client-id".to_owned()),
            mtn_client_secret: Some("client-secret".to_owned()),
            mtn_service_code: Some("131".to_owned()),
            mtn_tps_ceiling: Some(20.0),
            mtn_cost_per_segment_xaf: Some(rust_decimal::Decimal::new(15, 0)),
            ..base_cli()
        };
        let providers = build_provider_registry(&cli).unwrap();
        assert_eq!(providers.len(), 1);
        assert!(providers.contains_key("mtn_cm"));
    }

    /// The point of this whole PR's wiring: both adapters can be
    /// configured in the same process and both register, under distinct
    /// keys — proven directly against [`build_provider_registry`], not
    /// just asserted from reading `main`'s own two `if let` blocks.
    #[test]
    fn both_orange_and_mtn_register_together_under_distinct_keys() {
        let cli = Cli {
            orange_client_id: Some("id".to_owned()),
            orange_client_secret: Some("secret".to_owned()),
            orange_sender_number: Some("+237677000000".to_owned()),
            mtn_client_id: Some("client-id".to_owned()),
            mtn_client_secret: Some("client-secret".to_owned()),
            mtn_service_code: Some("131".to_owned()),
            mtn_sender_id: Some("SENDER".to_owned()),
            mtn_tps_ceiling: Some(20.0),
            mtn_cost_per_segment_xaf: Some(rust_decimal::Decimal::new(15, 0)),
            ..base_cli()
        };
        let providers = build_provider_registry(&cli).unwrap();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains_key("orange_cm"));
        assert!(providers.contains_key("mtn_cm"));
    }

    #[test]
    fn a_partial_orange_credential_set_is_rejected() {
        let cli = Cli {
            orange_client_id: Some("id".to_owned()),
            ..base_cli()
        };
        // `HashMap<String, Arc<dyn SmsProvider>>` (the `Ok` type) isn't
        // `Debug`, so `.unwrap_err()` doesn't compile here — `.err()`
        // sidesteps that: `Result::err` needs no bound on `T` at all.
        let error = build_provider_registry(&cli)
            .err()
            .expect("a partial Orange credential set must be rejected, not silently ignored");
        assert!(error.to_string().contains("orange-client-id"), "{error}");
    }

    #[test]
    fn a_partial_mtn_credential_set_is_rejected() {
        let cli = Cli {
            mtn_client_id: Some("client-id".to_owned()),
            mtn_tps_ceiling: Some(20.0),
            ..base_cli()
        };
        let error = build_provider_registry(&cli)
            .err()
            .expect("a partial MTN credential set must be rejected, not silently ignored");
        assert!(error.to_string().contains("mtn-client-id"), "{error}");
    }

    #[test]
    fn parses_the_deployment_docs_worked_example() {
        let raw = ["dispatch", "drain", "scheduler", "hooks", "jobs"].map(str::to_owned);
        let roles = parse_roles(&raw).unwrap();
        assert_eq!(
            roles,
            [
                Role::Dispatch,
                Role::Drain,
                Role::Scheduler,
                Role::Hooks,
                Role::Jobs
            ]
        );
    }

    #[test]
    fn parses_the_scaled_out_node_from_the_deployment_diagram() {
        let raw = ["hooks", "jobs"].map(str::to_owned);
        assert_eq!(parse_roles(&raw).unwrap(), [Role::Hooks, Role::Jobs]);
    }

    #[test]
    fn empty_roles_is_a_clear_error_not_a_silently_idle_process() {
        assert!(parse_roles(&[]).is_err());
    }

    #[test]
    fn an_unknown_role_name_fails_before_anything_is_spawned() {
        let raw = ["dispatch".to_owned(), "dispach".to_owned()];
        let err = parse_roles(&raw).unwrap_err();
        assert!(err.to_string().contains("dispach"), "{err}");
    }

    #[test]
    fn a_duplicated_role_is_rejected() {
        let raw = ["dispatch".to_owned(), "dispatch".to_owned()];
        let err = parse_roles(&raw).unwrap_err();
        assert!(err.to_string().contains("more than once"), "{err}");
    }

    #[test]
    fn whitespace_around_a_role_name_is_tolerated() {
        let raw = [" dispatch ".to_owned()];
        assert_eq!(parse_roles(&raw).unwrap(), [Role::Dispatch]);
    }
}
