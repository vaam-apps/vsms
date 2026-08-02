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
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// How long a shutdown waits for singleton roles to release their leases
/// before giving up and letting the process exit anyway. Generous relative
/// to a single `pg_advisory_unlock` round trip — this is a backstop against
/// a wedged connection, not the expected duration. If it fires, the lease's
/// `Drop` and Postgres's own session semantics still release the lock (see
/// `sms_worker::lease`'s module doc) — just via the slower path this exists
/// to avoid.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

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

/// A provider that panics if ever actually called — used only when
/// `dispatch` isn't among `--roles`, so `WorkerContext` can hold a plain
/// `Arc<dyn SmsProvider>` (no `Option`) uniformly across every role rather
/// than threading an `Option` through `run`'s signature for the one role
/// that needs it. The startup check above (`--roles` containing `dispatch`
/// requires real Orange credentials) is what makes "never called" an
/// actual guarantee here, not just a hope.
struct NoProviderConfigured;

#[async_trait::async_trait]
impl SmsProvider for NoProviderConfigured {
    fn key(&self) -> &str {
        unreachable!("dispatch is the only caller, and it requires real credentials at startup")
    }
    fn capabilities(&self) -> sms_provider::Capabilities {
        unreachable!("dispatch is the only caller, and it requires real credentials at startup")
    }
    async fn submit(
        &self,
        _req: &sms_provider::SubmitRequest,
    ) -> Result<sms_provider::SubmitAck, sms_provider::ProviderError> {
        unreachable!("dispatch is the only caller, and it requires real credentials at startup")
    }
    fn parse_dlr(
        &self,
        _raw: &sms_provider::RawCallback,
    ) -> Result<Vec<sms_provider::DeliveryUpdate>, sms_provider::ProviderError> {
        unreachable!("dispatch is the only caller, and it requires real credentials at startup")
    }
    async fn health(&self) -> sms_provider::Health {
        unreachable!("dispatch is the only caller, and it requires real credentials at startup")
    }
}

fn never_dispatched_provider() -> Arc<dyn SmsProvider> {
    Arc::new(NoProviderConfigured)
}

/// `hostname:pid` — stable enough to recognise a given process across a
/// restart in logs (the container id, under §9.2's Docker deployment)
/// without requiring every deploy to set `--worker-id` explicitly.
fn default_worker_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_owned());
    format!("{host}:{}", std::process::id())
}

#[tokio::main]
async fn main() -> Result<()> {
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

    let provider = orange_provider(&cli)?;
    if roles.contains(&Role::Dispatch) && provider.is_none() {
        bail!(
            "--roles includes dispatch, which needs --orange-client-id, \
             --orange-client-secret and --orange-sender-number (or their env vars) to submit \
             anything"
        );
    }
    // A lazy pool for roles that never end up needing a provider (every
    // role but `dispatch`, today) — constructing `WorkerContext`
    // unconditionally, the same way `Cli` requires `database_url`
    // unconditionally, keeps `run`'s signature uniform across roles rather
    // than threading an `Option` through every stub.
    let provider: Arc<dyn SmsProvider> = provider.unwrap_or_else(never_dispatched_provider);

    let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
        .max_connections(cli.db_max_connections)
        .connect(&cli.database_url)
        .await
        .context("connecting to Postgres")?;
    let ctx = WorkerContext {
        db: sms_api::schema::Cratestack::builder(pool).build(),
        provider,
    };
    let worker_id = cli.worker_id.clone().unwrap_or_else(default_worker_id);

    info!(
        roles = %roles.iter().copied().map(Role::as_str).collect::<Vec<_>>().join(","),
        worker_id,
        "sms-worker starting"
    );

    let shutdown = CancellationToken::new();
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
