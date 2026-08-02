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
use sms_worker::Role;
use std::collections::HashSet;
use tracing::info;

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

    let mut tasks = tokio::task::JoinSet::new();
    for role in roles {
        tasks.spawn(sms_worker::run(role));
    }

    tokio::select! {
        () = shutdown_signal() => {
            info!("shutdown signal received");
        }
        // A role task never returns on its own (sms_worker::run idles
        // forever) — so this branch firing means one panicked, which
        // JoinSet surfaces as an Err here rather than silently vanishing.
        Some(finished) = tasks.join_next() => {
            finished.context("a role task panicked")?;
            bail!("a role task returned, which sms_worker::run should never do");
        }
    }

    Ok(())
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

/// Resolve on SIGINT so in-flight work finishes.
///
/// Milestone 2 adds lease release here (§7.2: `Drop` cannot do it, releasing
/// an advisory lock needs an `await`) once #28 gives a singleton role
/// something to release.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
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
