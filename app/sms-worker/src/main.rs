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
use sms_worker::{Cardinality, Role};
use std::collections::HashSet;
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

    /// Only singleton roles use this today (#28's advisory lock). Required
    /// unconditionally anyway — a `--roles hooks,jobs`-only node not needing
    /// it yet is happenstance of what #29 hasn't built, not a property of
    /// this binary worth encoding as optional and re-deriving per role.
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
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

    info!(
        roles = %roles.iter().copied().map(Role::as_str).collect::<Vec<_>>().join(","),
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
                tasks.spawn(async move {
                    tokio::select! {
                        () = sms_worker::run(role) => {}
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
