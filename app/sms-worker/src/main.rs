//! The role-selectable worker binary. §7 of the design doc.
//!
//! This package is `sms-worker-bin`, not `sms-worker` — that name belongs to
//! the library crate this binary depends on (`crates/sms-worker`), and
//! Cargo package names must be unique workspace-wide. The `[[bin]]` override
//! in `Cargo.toml` is what makes the produced executable `sms-worker`
//! regardless, matching every `sms-worker --roles ...` example in the design
//! doc.

use anyhow::{bail, Context, Result};
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
/// `app/sms-worker/Dockerfile`) so a couple of missed ticks under load
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
/// this replaced (`app/sms-worker/Dockerfile`'s old `HEALTHCHECK`,
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
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,

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

    /// `OAuth2` `client_credentials` client id for Orange Cameroon's SMS
    /// API — required only when `dispatch` is one of `--roles`, since it's
    /// the only role that submits through a provider today (#33).
    #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
    orange_client_id: Option<String>,

    /// Paired with `orange_client_id` — see its doc for when this is
    /// required. Never logged; `OrangeCmConfig` holds it only long enough
    /// to fetch and cache a bearer token (`sms-provider-orange-cm`'s own
    /// `token` module).
    #[arg(long, env = "ORANGE_CM_CLIENT_SECRET")]
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

    /// `receiptRequest.notifyURL` on every submit (#95's DLR-correlation
    /// fix — see `sms-provider-orange-cm`'s `dlr.rs` module doc). Optional:
    /// Orange's own DLR webhook is documented elsewhere as "whitelisted per
    /// a manual support ticket," which reads as pre-registered rather than
    /// per-request, so `callbackData` (always sent, unconditionally) may be
    /// all correlation actually needs. Set this only if a real Orange
    /// sandbox turns out to require an explicit `notifyURL` too.
    #[arg(long, env = "ORANGE_CM_DLR_NOTIFY_URL")]
    orange_dlr_notify_url: Option<String>,
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
            config.dlr_notify_url.clone_from(&cli.orange_dlr_notify_url);
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

/// Build the [`sms_worker::ProviderRegistry`] this process holds
/// credentials for, keyed by each adapter's own [`SmsProvider::key`] — #62:
/// `dispatch::resolve_provider` looks a routed message's provider up by
/// exactly this string, matching `Provider.key` in the database. Empty
/// when `dispatch` isn't among `--roles` (nothing in any other role ever
/// reads this registry) — the startup check right after this call is what
/// makes "dispatch requires real credentials" an actual guarantee, not
/// just a hope. Only one entry exists today (`"orange_cm"`); a second real
/// adapter (#61) is a second `.insert(...)` here, not a redesign of this
/// function's shape.
fn build_provider_registry(cli: &Cli) -> Result<HashMap<String, Arc<dyn SmsProvider>>> {
    let mut providers: HashMap<String, Arc<dyn SmsProvider>> = HashMap::new();
    if let Some(orange) = orange_provider(cli)? {
        providers.insert(orange.key().to_owned(), orange);
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

/// This binary has no HTTP surface (six poll-loop roles, never a
/// listener — see this module's own doc), so there is nothing for a
/// `curl`-style container `HEALTHCHECK` to hit (#139). A heartbeat file is
/// the substitute: spawned unconditionally, independent of which `--roles`
/// this process runs, so it says something a bare `pgrep sms-worker` in the
/// `HEALTHCHECK` command could not — a hung tokio runtime (a role
/// deadlocked rather than merely idling on `run`'s own
/// `std::future::pending` stub, or a claim loop wedged on a connection)
/// stops touching this file even though the process is still very much
/// alive as far as `pgrep` is concerned.
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
    // `install_default_crypto_provider`'s own doc (app/sms-gateway/src/
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
            "--roles includes dispatch, which needs --orange-client-id, \
             --orange-client-secret and --orange-sender-number (or their env vars) to submit \
             anything"
        );
    }

    let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
        .max_connections(cli.db_max_connections)
        .connect(&cli.database_url)
        .await
        .context("connecting to Postgres")?;
    let ctx = WorkerContext {
        db: sms_api::schema::Cratestack::builder(pool).build(),
        providers: Arc::new(providers),
    };

    // Unconditional — not gated on `drain` being one of `--roles`. Every
    // role in this process shares this one `Cratestack` (cloned per task,
    // sharing the same underlying `CoolEventBus` via its `Arc`-backed
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
/// see `app/sms-gateway/src/main.rs`'s identical function for the full
/// reasoning (`authkestra-*`'s `rustls-no-provider` feature, the musl
/// build it unblocks, why `ring` and not `aws-lc-rs`). `.ok()`, not
/// `.expect(...)`, for the same reason as that copy: the only failure mode
/// is "already installed," never a reason to abort startup.
fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The `sms-worker healthcheck` exec-form check — see `main`'s own comment
/// for why this is intercepted ahead of `Cli::parse()` rather than a real
/// clap subcommand, and `app/sms-worker/Dockerfile`'s header for why an
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
        use tokio::signal::unix::{signal, SignalKind};
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
    use super::parse_roles;
    use sms_worker::Role;

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
