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

    /// The YAML deployment configuration (VSC-1): the pool size and every
    /// provider's credentials, which the gateway reads from the same document.
    #[command(flatten)]
    config: sms_config::ConfigArgs,
}

/// The adapters this process holds credentials for, by `Provider.key`.
type ProviderRegistry = HashMap<String, Arc<dyn SmsProvider>>;

/// Build the [`sms_worker::ProviderRegistry`] this process holds
/// credentials for, keyed by each adapter's own [`SmsProvider::key`] — #62:
/// `dispatch::resolve_provider` looks a routed message's provider up by
/// exactly this string, matching `Provider.key` in the database. A provider
/// is in the registry when the YAML has a `providers` entry for it (and, for
/// `aws_sms`, the entry's sending keys: an entry that only receives is the
/// gateway's). Either, any or none may be configured; a fourth real adapter is
/// a fourth `if let` here, not a redesign of this function's shape. The
/// conversions to each adapter's config are `sms_config`'s, shared with the
/// gateway, and the all-or-none checks the flags needed are the loader's.
///
/// # Errors
///
/// The AWS adapter's own range checks (region shape, time to live) on a
/// configuration the loader accepted.
fn build_provider_registry(providers: &sms_config::Providers) -> Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    if let Some(orange) = &providers.orange_cm {
        let provider: Arc<dyn SmsProvider> = Arc::new(
            sms_provider_orange_cm::OrangeCmProvider::new(orange.to_provider_config()),
        );
        registry.insert(provider.key().to_owned(), provider);
    }
    if let Some(mtn) = &providers.mtn_cm {
        let provider: Arc<dyn SmsProvider> =
            Arc::new(sms_provider_mtn::MtnProvider::new(mtn.to_provider_config()));
        registry.insert(provider.key().to_owned(), provider);
    }
    if let Some(sender) = providers
        .aws_sms
        .as_ref()
        .and_then(|aws| aws.sender.as_ref())
    {
        let provider: Arc<dyn SmsProvider> = Arc::new(
            sms_provider_aws_sms::AwsSmsProvider::sender(sender.to_provider_config())
                .context("providers[code=aws_sms]")?,
        );
        registry.insert(provider.key().to_owned(), provider);
    }
    Ok(registry)
}

/// The sealer `hooks` delivers `sealed` endpoints with, built from
/// `--envelope-signing-key-file`.
///
/// `None` (and a warning) when `hooks` runs without the flag: the role starts and
/// delivers every `hmac` endpoint, rather than a process that refuses to start for
/// a key an HMAC-only deployment never uses (VS-8R). It claims no `sealed`
/// endpoint's attempt, which waits, untouched, for a worker that holds the key;
/// the role says how many wait, naming the flag, every five minutes
/// (`sms_worker::hooks::SealedBacklogWatch`). A key that is given but cannot
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
             `hmac` webhook endpoints are delivered, and this worker claims no `sealed` \
             endpoint's attempt (they wait without spending an attempt, for a worker that \
             holds the gateway's PKCS#8 PEM Ed25519 signing key)"
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

/// Reads the YAML configuration (VSC-1) and builds the provider registry from
/// it, before anything is connected to: a retired variable, an unset `${VAR}` or
/// a provider with a key missing stops the process here, naming the YAML key.
/// Pulled out of `main` for clippy's `too_many_lines`, the convention this
/// binary and the gateway's `serve` share for a block that is self-contained.
///
/// # Errors
///
/// The configuration does not load, an adapter refuses its settings, or
/// `dispatch` is selected and no provider is configured to submit through.
fn load_configuration(cli: &Cli, roles: &[Role]) -> Result<(sms_config::Config, ProviderRegistry)> {
    let config = cli
        .config
        .load()
        .context("loading the YAML configuration")?;
    info!(
        deployment = %config.deployment.name,
        livemode = config.deployment.livemode,
        profile = %cli.config.profile,
        "configuration loaded"
    );

    let providers = build_provider_registry(&config.providers)?;
    if roles.contains(&Role::Dispatch) && providers.is_empty() {
        bail!(
            "--roles includes dispatch, which needs at least one provider to submit anything: \
             a `providers` entry with `code: orange_cm` or `code: mtn_cm`, or `code: aws_sms` \
             with its sending keys (region, configuration_set, tps_ceiling, \
             cost_per_segment_xaf, credentials) in config/application*.yml"
        );
    }
    Ok((config, providers))
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

    // VSC-1: before clap parses, so a flag that moved into the YAML is refused
    // naming the key that replaced it, not with "unexpected argument".
    sms_config::reject_retired_flags(std::env::args_os().skip(1))?;

    let cli = Cli::parse();
    let roles = parse_roles(&cli.roles)?;
    let (config, providers) = load_configuration(&cli, &roles)?;

    let webhook_sealer = webhook_sealer(&cli, &roles)?;

    let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.database.worker_max_connections)
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
    use clap::CommandFactory;
    use sms_config::{AwsSender, AwsSms, MtnCm, OrangeCm, Providers, Secret};
    use sms_worker::Role;

    /// Every field explicitly, not `Default` (there is none) — a minimal
    /// `Cli` a test can override individual fields on via struct-update syntax.
    fn base_cli() -> Cli {
        Cli {
            roles: Vec::new(),
            database_url: String::new(),
            metrics_listen: String::new(),
            worker_id: None,
            envelope_signing_key_file: None,
            config: sms_config::ConfigArgs {
                config: None,
                profile: "dev".to_owned(),
            },
        }
    }

    fn secret(text: &str) -> Secret {
        Secret::new(text)
    }

    fn orange() -> OrangeCm {
        OrangeCm {
            client_id: "id".to_owned(),
            client_secret: secret("secret"),
            sender_number: "+237677000000".to_owned(),
            base_url: sms_config::ORANGE_DEFAULT_BASE_URL.to_owned(),
        }
    }

    fn mtn() -> MtnCm {
        MtnCm {
            client_id: "client-id".to_owned(),
            client_secret: secret("client-secret"),
            service_code: "131".to_owned(),
            sender_id: Some("SENDER".to_owned()),
            base_url: sms_config::MTN_DEFAULT_BASE_URL.to_owned(),
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(15, 0),
            supports_alphanumeric_sender: false,
        }
    }

    fn aws_sender() -> AwsSender {
        AwsSender {
            region: "eu-west-1".to_owned(),
            access_key_id: "AKIDEXAMPLE".to_owned(),
            secret_access_key: secret("secret"),
            configuration_set: "vsms-events".to_owned(),
            origination_identity: None,
            message_type: sms_provider_aws_sms::MessageType::Transactional,
            tps_ceiling: 20.0,
            cost_per_segment_xaf: rust_decimal::Decimal::new(30, 0),
            supports_alphanumeric_sender: false,
            endpoint_url: None,
            time_to_live: None,
        }
    }

    fn aws(sender: Option<AwsSender>) -> AwsSms {
        AwsSms {
            sender,
            events_topic_arn: None,
            fake_sns_cert_base: None,
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
        let providers = Providers {
            aws_sms: Some(aws(Some(aws_sender()))),
            ..Providers::default()
        };
        let registry = build_provider_registry(&providers).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.contains_key("aws_sms"));
    }

    /// The later shape the routing weights split between: every adapter in
    /// one process, each under its own key.
    #[test]
    fn aws_sms_registers_alongside_orange_and_mtn() {
        let providers = Providers {
            orange_cm: Some(orange()),
            mtn_cm: Some(mtn()),
            aws_sms: Some(aws(Some(aws_sender()))),
        };
        let registry = build_provider_registry(&providers).unwrap();
        let mut keys: Vec<_> = registry.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["aws_sms", "mtn_cm", "orange_cm"]);
    }

    /// An `aws_sms` entry that only receives is the gateway's: the worker has
    /// nothing to send through and registers nothing for it.
    #[test]
    fn an_aws_entry_that_only_receives_registers_no_sender() {
        let providers = Providers {
            aws_sms: Some(aws(None)),
            ..Providers::default()
        };
        assert!(build_provider_registry(&providers).unwrap().is_empty());
    }

    #[test]
    fn an_aws_sms_configuration_that_could_never_send_is_an_error() {
        let providers = Providers {
            aws_sms: Some(aws(Some(AwsSender {
                region: "Not A Region".to_owned(),
                ..aws_sender()
            }))),
            ..Providers::default()
        };
        assert!(build_provider_registry(&providers).is_err());
    }

    #[test]
    fn the_aws_sms_time_to_live_is_optional_and_range_checked() {
        let with = |seconds| Providers {
            aws_sms: Some(aws(Some(AwsSender {
                time_to_live: Some(std::time::Duration::from_secs(seconds)),
                ..aws_sender()
            }))),
            ..Providers::default()
        };
        assert!(build_provider_registry(&with(900)).is_ok());
        assert!(build_provider_registry(&with(4)).is_err());
        assert!(build_provider_registry(&with(259_201)).is_err());
    }

    #[test]
    fn no_provider_configured_is_an_empty_registry_not_an_error() {
        let registry = build_provider_registry(&Providers::default()).unwrap();
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn orange_alone_registers_under_its_own_key() {
        let providers = Providers {
            orange_cm: Some(orange()),
            ..Providers::default()
        };
        let registry = build_provider_registry(&providers).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.contains_key("orange_cm"));
    }

    #[test]
    fn mtn_alone_registers_under_its_own_key() {
        let providers = Providers {
            mtn_cm: Some(mtn()),
            ..Providers::default()
        };
        let registry = build_provider_registry(&providers).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.contains_key("mtn_cm"));
    }

    /// Both adapters can be configured in the same process and both register,
    /// under distinct keys — proven directly against [`build_provider_registry`],
    /// not just asserted from reading its own `if let` blocks.
    #[test]
    fn both_orange_and_mtn_register_together_under_distinct_keys() {
        let providers = Providers {
            orange_cm: Some(orange()),
            mtn_cm: Some(mtn()),
            ..Providers::default()
        };
        let registry = build_provider_registry(&providers).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.contains_key("orange_cm"));
        assert!(registry.contains_key("mtn_cm"));
    }

    /// The flags VSC-1 moved into the YAML are not flags any more, and none is
    /// bound to its old variable: clap would otherwise read the variable the
    /// retired-variable check exists to refuse. Re-adding any one of them fails
    /// here.
    #[test]
    fn the_worker_still_declares_no_retired_flag_or_variable() {
        let command = Cli::command();
        for arg in command.get_arguments() {
            if let Some(long) = arg.get_long() {
                assert!(
                    !sms_config::RETIRED
                        .iter()
                        .any(|retired| retired.flag == format!("--{long}")),
                    "--{long} is retired (sms_config::RETIRED) and is declared again"
                );
            }
            if let Some(env) = arg.get_env().and_then(|env| env.to_str()) {
                assert!(
                    !sms_config::RETIRED.iter().any(|retired| retired.env == env),
                    "{env} is retired (sms_config::RETIRED) and is bound to an argument again"
                );
            }
        }
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
